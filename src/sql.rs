//! `std.sql` — SQLite (rusqlite WAL) bridge for Lua scripts.
//!
//! Provides:
//! - `std.sql.query(sql, params?) -> rows`   rows = array of { col_name = value, ... }
//! - `std.sql.exec(sql, params?)  -> { affected = N, last_id = M }`
//! - `std.sql.null` — sentinel for SQL NULL on the Lua side
//!
//! Statements run on the connection thread owned by an
//! [`AsyncIsle`](crate::sqlite_backend::rusqlite_isle::AsyncIsle), so no
//! blocking call and no lock guard ever crosses an `.await` in this crate.
//!
//! # Wiring contract
//!
//! The host owns the isle (file path / `busy_timeout` / `journal_mode` are
//! host-side concerns, applied in the isle's `init` closure or through
//! `AsyncIsleBuilder::wal`) and its `AsyncIsleDriver`, which is what shuts
//! the connection thread down.  Pass a clone of the handle to [`register`] /
//! [`register_with`].  This crate does not open the database, does not read
//! environment variables, and does not attempt to recover from a corrupt
//! connection.
//!
//! Which `rusqlite` / `rusqlite-isle` versions those types come from is
//! selected by the track feature — see [`crate::sqlite_backend`].
//!
//! # Cancellation integration
//!
//! When the `task` feature is enabled (it is, transitively, since `sql`
//! depends on `task`), every query/exec races against the enclosing
//! `task.scope` / `task.with_timeout`'s [`CancelToken`](crate::task::CancelToken)
//! via [`crate::task::effective_token`].  Jobs are submitted with
//! `spawn_call`, whose `AsyncTask` cancels on drop: when the enclosing token
//! fires, or the configured timeout elapses, the dropped task interrupts the
//! running statement through the isle's own two-stage cancellation.

use std::time::Duration;

use mlua::prelude::*;
use serde_json::Map;
use tracing::warn;

use crate::sqlite_backend::rusqlite::{
    self,
    types::{Value, ValueRef},
    Connection,
};
use crate::sqlite_backend::rusqlite_isle::{AsyncIsle, IsleError};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Runtime configuration for the SQL/KV bridges.
///
/// Stored in `lua.app_data` by [`register_with`] and consulted by
/// [`race_timeout`] for the per-query timeout.  Shared between `std.sql`
/// and `std.kv` since both speak to a SQLite connection with identical
/// timeout semantics.
#[derive(Clone, Debug)]
pub struct SqlConfig {
    /// Per-query timeout.  `None` disables the timeout (the operation
    /// runs until completion or until the enclosing task cancels).
    pub query_timeout: Option<Duration>,
}

impl Default for SqlConfig {
    fn default() -> Self {
        Self {
            query_timeout: Some(Duration::from_millis(5000)),
        }
    }
}

/// Register `std.sql` with default [`SqlConfig`].
pub fn register(lua: &Lua, isle: AsyncIsle) -> LuaResult<()> {
    register_with(lua, isle, SqlConfig::default())
}

/// Register `std.sql` with caller-provided [`SqlConfig`].
///
/// The config is stored in `lua.app_data`.  `std.kv` (registered via
/// [`crate::kv::register_with`]) shares the same `SqlConfig` slot, so
/// calling either `register_with` after the other replaces the previous
/// config — pass identical configs from the host or only set it once.
pub fn register_with(lua: &Lua, isle: AsyncIsle, cfg: SqlConfig) -> LuaResult<()> {
    lua.set_app_data::<SqlConfig>(cfg);

    let sql_tbl = lua.create_table()?;

    // ── std.sql.null ──────────────────────────────────────────────────────
    // Sentinel that represents SQL NULL on the Lua side (also used for JSON
    // null in values returned from `sql` / `kv` / other bridges).
    // `mlua::Value::NULL` is `LightUserData(null_ptr)`, and any equivalent
    // LightUserData produced from `std::ptr::null_mut()` compares equal via
    // Lua `==` (lightuserdata equality is pointer equality), so scripts can
    // write `if row.col == std.sql.null then ... end`.
    sql_tbl.set("null", LuaValue::NULL)?;

    // ── std.sql.query ─────────────────────────────────────────────────────
    {
        let isle = isle.clone();
        sql_tbl.set(
            "query",
            lua.create_async_function(move |lua, (sql, params): (String, Option<LuaTable>)| {
                let isle = isle.clone();
                let params_result = params
                    .map(|t| lua_params_to_values(&t))
                    .transpose()
                    .map_err(LuaError::external);
                async move {
                    let params_vec = params_result?.unwrap_or_default();
                    let timeout = sql_query_timeout(&lua);
                    let rows = run_job(&isle, timeout, "sql.query", move |conn| {
                        Ok(run_query(conn, &sql, &params_vec))
                    })
                    .await?;
                    rows_to_lua(&lua, rows)
                }
            })?,
        )?;
    }

    // ── std.sql.exec ──────────────────────────────────────────────────────
    {
        let isle = isle.clone();
        sql_tbl.set(
            "exec",
            lua.create_async_function(move |lua, (sql, params): (String, Option<LuaTable>)| {
                let isle = isle.clone();
                let params_result = params
                    .map(|t| lua_params_to_values(&t))
                    .transpose()
                    .map_err(LuaError::external);
                async move {
                    let params_vec = params_result?.unwrap_or_default();
                    let timeout = sql_query_timeout(&lua);
                    let (affected, last_id) =
                        run_job(&isle, timeout, "sql.exec", move |conn| {
                            Ok(run_exec(conn, &sql, &params_vec))
                        })
                        .await?;

                    let result_tbl = lua.create_table()?;
                    result_tbl.set("affected", affected as i64)?;
                    result_tbl.set("last_id", last_id)?;
                    Ok(LuaValue::Table(result_tbl))
                }
            })?,
        )?;
    }

    let std_ns: LuaTable = lua.globals().get("std")?;
    std_ns.set("sql", sql_tbl)?;
    Ok(())
}

pub(crate) fn sql_query_timeout(lua: &Lua) -> Option<Duration> {
    lua.app_data_ref::<SqlConfig>()
        .map(|c| c.query_timeout)
        .unwrap_or_else(|| SqlConfig::default().query_timeout)
}

// ---------------------------------------------------------------------------
// Helpers shared with `std.kv` (re-exported under `pub(crate)`)
// ---------------------------------------------------------------------------

/// Submit one job to the isle and race it against (a) the enclosing task's
/// cancel token and (b) the configured query timeout.
///
/// The job is submitted with `spawn_call`, whose `AsyncTask` cancels on
/// drop.  Both losing branches therefore drop the task, which drains it from
/// the queue if it has not started and interrupts the running statement if it
/// has — the isle owns the `InterruptHandle`, so this crate no longer needs
/// one.
///
/// The job returns `Result<Result<T, String>, rusqlite::Error>`: the inner
/// `String` carries the bridge's own diagnostics (unsupported column type,
/// non-UTF-8 TEXT, …), while the outer `rusqlite::Error` slot is left for the
/// isle's error normalization.
///
/// # Threading model
///
/// The returned future is `!Send`: the cancel token held by
/// `effective_token()` is `Rc<_>`, and the whole bridge surface is
/// single-threaded by design.  Callers must `.await` this future on the
/// same `LocalSet` that owns the VM; wrapping it in `tokio::spawn` will
/// fail to compile.
pub(crate) async fn run_job<T, F>(
    isle: &AsyncIsle,
    timeout: Option<Duration>,
    op: &'static str,
    job: F,
) -> LuaResult<T>
where
    T: Send + 'static,
    F: FnOnce(&mut Connection) -> Result<Result<T, String>, rusqlite::Error> + Send + 'static,
{
    let task = isle.spawn_call(job);

    let wait = async {
        match timeout {
            Some(d) => match tokio::time::timeout(d, task).await {
                Ok(r) => Ok(r),
                Err(_) => Err(d),
            },
            None => Ok(task.await),
        }
    };

    let wait_result = match crate::task::effective_token() {
        Some(t) => tokio::select! {
            biased;
            _ = t.cancelled() => {
                warn!(op, "cancelled by enclosing task");
                return Err(LuaError::external(format!(
                    "task cancelled during {op}"
                )));
            }
            r = wait => r,
        },
        None => wait.await,
    };

    let job_result = match wait_result {
        Ok(r) => r,
        Err(d) => {
            warn!(op, timeout_ms = d.as_millis() as u64, "operation timeout");
            return Err(LuaError::external(format!(
                "{op} timeout ({}ms)",
                d.as_millis()
            )));
        }
    };

    match job_result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => {
            warn!(op, error = %e, "execution error");
            Err(LuaError::external(e))
        }
        Err(IsleError::Cancelled) => {
            warn!(op, "cancelled by the isle");
            Err(LuaError::external(format!("task cancelled during {op}")))
        }
        Err(e) => {
            warn!(op, error = %e, "isle error");
            Err(LuaError::external(format!("{op}: {e}")))
        }
    }
}

// ---------------------------------------------------------------------------
// Param conversion: Lua → rusqlite
// ---------------------------------------------------------------------------

/// Convert a Lua array table to `Vec<rusqlite::types::Value>`.
fn lua_params_to_values(tbl: &LuaTable) -> Result<Vec<Value>, String> {
    let len = tbl.raw_len();
    let mut result = Vec::with_capacity(len);
    for i in 1..=len {
        let v: LuaValue = tbl
            .raw_get(i)
            .map_err(|e| format!("params table access error: {e}"))?;
        let sql_val = match v {
            LuaValue::Nil => Value::Null,
            LuaValue::Boolean(b) => Value::Integer(if b { 1 } else { 0 }),
            LuaValue::Integer(n) => Value::Integer(n),
            LuaValue::Number(f) => {
                if !f.is_finite() {
                    return Err(format!(
                        "SQL param #{i} is non-finite ({f}); NaN and ±Inf are not supported"
                    ));
                }
                Value::Real(f)
            }
            LuaValue::String(s) => Value::Text(
                s.to_str()
                    .map_err(|e| format!("param string encoding error: {e}"))?
                    .to_string(),
            ),
            other => return Err(format!("unsupported SQL param type: {}", other.type_name())),
        };
        result.push(sql_val);
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Query/Exec execution
// ---------------------------------------------------------------------------

fn run_query(
    conn: &Connection,
    sql: &str,
    params: &[Value],
) -> Result<Vec<Map<String, serde_json::Value>>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| format!("sql error: {e}"))?;

    let col_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

    let mut rows = stmt
        .query(rusqlite::params_from_iter(params.iter()))
        .map_err(|e| format!("sql error: {e}"))?;

    let mut result = Vec::new();
    while let Some(row) = rows.next().map_err(|e| format!("sql error: {e}"))? {
        let mut map = serde_json::Map::new();
        for (i, name) in col_names.iter().enumerate() {
            let val = match row.get_ref(i).map_err(|e| format!("sql error: {e}"))? {
                ValueRef::Null => serde_json::Value::Null,
                ValueRef::Integer(n) => serde_json::Value::Number(n.into()),
                ValueRef::Real(f) => serde_json::Number::from_f64(f)
                    .map(serde_json::Value::Number)
                    .ok_or_else(|| {
                        format!(
                            "non-finite REAL in column '{}' ({f}); \
                             NaN / ±Inf cannot be represented in JSON/Lua",
                            col_names[i]
                        )
                    })?,
                ValueRef::Text(b) => {
                    let s = std::str::from_utf8(b)
                        .map_err(|e| format!("non-UTF-8 TEXT in column '{}': {e}", col_names[i]))?;
                    serde_json::Value::String(s.to_string())
                }
                ValueRef::Blob(_) => return Err("blob columns not supported".to_string()),
            };
            map.insert(name.clone(), val);
        }
        result.push(map);
    }

    Ok(result)
}

fn run_exec(conn: &Connection, sql: &str, params: &[Value]) -> Result<(usize, i64), String> {
    let affected = conn
        .execute(sql, rusqlite::params_from_iter(params.iter()))
        .map_err(|e| format!("sql error: {e}"))?;
    let last_id = conn.last_insert_rowid();
    Ok((affected, last_id))
}

// ---------------------------------------------------------------------------
// Row → Lua conversion (NULL-preserving variant)
// ---------------------------------------------------------------------------

/// Convert a list of column-name→JSON-value maps into a Lua array table.
///
/// NULL columns arrive as `serde_json::Value::Null` and are translated by
/// [`json_to_lua_preserving_null`] into the `LightUserData(null_ptr)` sentinel
/// (exposed to Lua as `std.sql.null`), which keeps the column present in
/// the row table.  This preserves the distinction between "column is NULL"
/// and "column was not in the query".
///
/// A zero-row result carries the shared `__jsontype = "array"` metatable so
/// that `json.encode(rows)` renders `[]` rather than `{}`.
pub(crate) fn rows_to_lua(
    lua: &Lua,
    rows: Vec<Map<String, serde_json::Value>>,
) -> LuaResult<LuaValue> {
    let arr = lua.create_table()?;
    let row_count = rows.len();
    for (i, row_map) in rows.into_iter().enumerate() {
        let row_tbl = lua.create_table()?;
        for (col, val) in row_map {
            let lua_val = json_to_lua_preserving_null(lua, val)?;
            row_tbl.set(col.as_str(), lua_val)?;
        }
        arr.set(i + 1, row_tbl)?;
    }
    if row_count == 0 {
        arr.set_metatable(Some(crate::json::array_metatable(lua)?))?;
    }
    Ok(LuaValue::Table(arr))
}

// ---------------------------------------------------------------------------
// JSON ↔ Lua helpers (NULL-preserving — distinct from crate::json)
//
// `crate::json` lowers JSON `null` to Lua `nil` (idiomatic for
// `std.json.decode`).  The sql/kv bridges instead need round-trip fidelity
// so SQL NULL columns and JSON `null` values survive being placed into Lua
// tables (which cannot hold `nil`).  Agents compare against `std.sql.null`.
// ---------------------------------------------------------------------------

const MAX_JSON_DEPTH: usize = 128;

pub(crate) fn json_to_lua_preserving_null(
    lua: &Lua,
    val: serde_json::Value,
) -> LuaResult<LuaValue> {
    json_to_lua_inner(lua, &val, 0)
}

fn json_to_lua_inner(lua: &Lua, val: &serde_json::Value, depth: usize) -> LuaResult<LuaValue> {
    if depth > MAX_JSON_DEPTH {
        return Err(LuaError::external(format!(
            "JSON nesting too deep (limit: {MAX_JSON_DEPTH})"
        )));
    }
    match val {
        serde_json::Value::Null => Ok(LuaValue::NULL),
        serde_json::Value::Bool(b) => Ok(LuaValue::Boolean(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(LuaValue::Integer(i))
            } else if let Some(f) = n.as_f64() {
                Ok(LuaValue::Number(f))
            } else {
                Err(LuaError::external(format!(
                    "JSON number {n} is not representable as i64 or f64"
                )))
            }
        }
        serde_json::Value::String(s) => lua.create_string(s).map(LuaValue::String),
        serde_json::Value::Array(arr) => {
            let table = lua.create_table()?;
            for (i, v) in arr.iter().enumerate() {
                table.set(i + 1, json_to_lua_inner(lua, v, depth + 1)?)?;
            }
            if arr.is_empty() {
                // Same `__jsontype = "array"` tag (and the same metatable
                // instance) that `crate::json` uses, so an empty array read
                // back out of sql / kv re-encodes as `[]` rather than `{}`.
                table.set_metatable(Some(crate::json::array_metatable(lua)?))?;
            }
            Ok(LuaValue::Table(table))
        }
        serde_json::Value::Object(map) => {
            let table = lua.create_table()?;
            for (k, v) in map {
                table.set(k.as_str(), json_to_lua_inner(lua, v, depth + 1)?)?;
            }
            Ok(LuaValue::Table(table))
        }
    }
}

pub(crate) fn lua_to_json_preserving_null(val: LuaValue) -> LuaResult<serde_json::Value> {
    lua_to_json_inner(&val, 0)
}

fn lua_to_json_inner(val: &LuaValue, depth: usize) -> LuaResult<serde_json::Value> {
    if depth > MAX_JSON_DEPTH {
        return Err(LuaError::external(format!(
            "Lua table nesting too deep for JSON (limit: {MAX_JSON_DEPTH})"
        )));
    }
    match val {
        LuaValue::Nil => Ok(serde_json::Value::Null),
        LuaValue::LightUserData(u) if u.0.is_null() => Ok(serde_json::Value::Null),
        LuaValue::Boolean(b) => Ok(serde_json::Value::Bool(*b)),
        LuaValue::Integer(i) => Ok(serde_json::Value::Number((*i).into())),
        LuaValue::Number(n) => serde_json::Number::from_f64(*n)
            .map(serde_json::Value::Number)
            .ok_or_else(|| LuaError::external(format!("cannot convert {n} to JSON number"))),
        LuaValue::String(s) => Ok(serde_json::Value::String(s.to_str()?.to_string())),
        LuaValue::Table(t) => {
            let len = t.raw_len();
            if len > 0 {
                let mut arr = Vec::with_capacity(len);
                for i in 1..=len {
                    let v: LuaValue = t.raw_get(i)?;
                    arr.push(lua_to_json_inner(&v, depth + 1)?);
                }
                Ok(serde_json::Value::Array(arr))
            } else {
                let mut map = serde_json::Map::new();
                for pair in t.clone().pairs::<LuaValue, LuaValue>() {
                    let (k, v) = pair?;
                    let key = match k {
                        LuaValue::String(s) => s.to_str()?.to_string(),
                        LuaValue::Integer(i) => i.to_string(),
                        LuaValue::Number(n) => n.to_string(),
                        other => {
                            return Err(LuaError::external(format!(
                                "unsupported table key type for JSON: {}",
                                other.type_name()
                            )));
                        }
                    };
                    map.insert(key, lua_to_json_inner(&v, depth + 1)?);
                }
                if map.is_empty() && crate::json::wants_empty_array(t)? {
                    // Empty table asking to be an array (`__jsontype` tag or
                    // mlua's serde array metatable) — same rule as
                    // `crate::json`.  Tables with entries never get here.
                    return Ok(serde_json::Value::Array(Vec::new()));
                }
                Ok(serde_json::Value::Object(map))
            }
        }
        other => Err(LuaError::external(format!(
            "unsupported type for JSON conversion: {}",
            other.type_name()
        ))),
    }
}
