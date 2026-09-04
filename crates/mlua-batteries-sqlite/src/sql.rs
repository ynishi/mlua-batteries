//! `std.sql` — SQLite (rusqlite WAL) bridge for Lua scripts.
//!
//! Provides:
//! - `std.sql.query(sql, params?) -> rows`   rows = array of { col_name = value, ... }
//! - `std.sql.exec(sql, params?)  -> { affected = N, last_id = M }`
//! - `std.sql.null` — sentinel for SQL NULL on the Lua side
//!
//! rusqlite calls are executed inside `tokio::task::spawn_blocking` to avoid
//! blocking the async runtime.  Lock acquisition is also inside spawn_blocking
//! to prevent holding a Mutex guard across `.await` (await-holding-lock).
//!
//! # Wiring contract
//!
//! The host owns the [`rusqlite::Connection`] (file path / `busy_timeout` /
//! `journal_mode` are host-side concerns, not this crate's) and its
//! [`rusqlite::InterruptHandle`].  Pass them to [`register`] /
//! [`register_with`] wrapped in `Arc<Mutex<_>>` / `Arc<_>`.  This crate
//! does not open the database, does not read environment variables, and
//! does not attempt to recover from a corrupt connection.
//!
//! Which `rusqlite` those types come from is decided by this crate's
//! dependency — see [`crate::rusqlite`] to name it without a second
//! dependency that could drift onto another `libsqlite3-sys` cluster.
//!
//! # Cancellation integration
//!
//! Every query/exec races against the enclosing `task.scope` /
//! `task.with_timeout`'s
//! [`CancelToken`](mlua_batteries::task::CancelToken) via
//! [`mlua_batteries::task::effective_token`].  When the token fires we call
//! `sqlite3_interrupt` so the blocking thread returns quickly and the
//! Mutex guard is released.

use std::sync::Arc;
use std::time::Duration;

use mlua::prelude::*;
use mlua_batteries::json::{array_metatable, json_to_lua_preserving_null};
use serde_json::Map;
use tracing::warn;

use crate::sqlite_backend::rusqlite::{
    self,
    types::{Value, ValueRef},
    Connection, InterruptHandle,
};

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
pub fn register(
    lua: &Lua,
    conn: Arc<std::sync::Mutex<Connection>>,
    interrupt: Arc<InterruptHandle>,
) -> LuaResult<()> {
    register_with(lua, conn, interrupt, SqlConfig::default())
}

/// Register `std.sql` with caller-provided [`SqlConfig`].
///
/// The config is stored in `lua.app_data`.  `std.kv` (registered via
/// [`crate::kv::register_with`]) shares the same `SqlConfig` slot, so
/// calling either `register_with` after the other replaces the previous
/// config — pass identical configs from the host or only set it once.
pub fn register_with(
    lua: &Lua,
    conn: Arc<std::sync::Mutex<Connection>>,
    interrupt: Arc<InterruptHandle>,
    cfg: SqlConfig,
) -> LuaResult<()> {
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
        let conn = Arc::clone(&conn);
        let interrupt = Arc::clone(&interrupt);
        sql_tbl.set(
            "query",
            lua.create_async_function(move |lua, (sql, params): (String, Option<LuaTable>)| {
                let conn = Arc::clone(&conn);
                let interrupt = Arc::clone(&interrupt);
                let params_result = params
                    .map(|t| lua_params_to_values(&t))
                    .transpose()
                    .map_err(LuaError::external);
                async move {
                    let params_vec = params_result?.unwrap_or_default();
                    let fut = tokio::task::spawn_blocking(move || {
                        let guard = lock_conn(&conn);
                        run_query(&guard, &sql, &params_vec)
                    });
                    let timeout = sql_query_timeout(&lua);
                    let rows = race_timeout(fut, timeout, &interrupt, "sql.query").await?;
                    rows_to_lua(&lua, rows)
                }
            })?,
        )?;
    }

    // ── std.sql.exec ──────────────────────────────────────────────────────
    {
        let conn = Arc::clone(&conn);
        let interrupt = Arc::clone(&interrupt);
        sql_tbl.set(
            "exec",
            lua.create_async_function(move |lua, (sql, params): (String, Option<LuaTable>)| {
                let conn = Arc::clone(&conn);
                let interrupt = Arc::clone(&interrupt);
                let params_result = params
                    .map(|t| lua_params_to_values(&t))
                    .transpose()
                    .map_err(LuaError::external);
                async move {
                    let params_vec = params_result?.unwrap_or_default();
                    let fut = tokio::task::spawn_blocking(move || {
                        let guard = lock_conn(&conn);
                        run_exec(&guard, &sql, &params_vec)
                    });
                    let timeout = sql_query_timeout(&lua);
                    let (affected, last_id) =
                        race_timeout(fut, timeout, &interrupt, "sql.exec").await?;

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

/// Lock the shared Connection mutex without panicking.
///
/// On `PoisonError` we log and recover via `into_inner()`. Poison here means a
/// previous blocking thread panicked while holding the guard; for a local
/// agent-runtime SQLite (single-process, embedded) the safest path is to log
/// and keep serving rather than tear the host down.
pub(crate) fn lock_conn(
    conn: &std::sync::Mutex<Connection>,
) -> std::sync::MutexGuard<'_, Connection> {
    conn.lock().unwrap_or_else(|poisoned| {
        warn!("sql conn mutex was poisoned; recovering via into_inner");
        poisoned.into_inner()
    })
}

/// Race an `spawn_blocking` SQL operation against (a) the enclosing task's
/// cancel token and (b) the configured query timeout.
///
/// When either fires first we call `sqlite3_interrupt` via the stored handle
/// so the blocking thread returns quickly, releases the Mutex guard, and
/// frees the connection for subsequent calls.
///
/// # Threading model
///
/// The returned future is `!Send`: the cancel token held by
/// `effective_token()` is `Rc<_>`, and the whole bridge surface is
/// single-threaded by design.  Callers must `.await` this future on the
/// same `LocalSet` that owns the VM; wrapping it in `tokio::spawn` will
/// fail to compile.
pub(crate) async fn race_timeout<T, F>(
    fut: F,
    timeout: Option<Duration>,
    interrupt: &InterruptHandle,
    op: &'static str,
) -> LuaResult<T>
where
    F: std::future::Future<Output = Result<Result<T, String>, tokio::task::JoinError>>,
{
    let wait = async {
        match timeout {
            Some(d) => match tokio::time::timeout(d, fut).await {
                Ok(j) => Ok(j),
                Err(_) => Err(d),
            },
            None => Ok(fut.await),
        }
    };

    let wait_result = match mlua_batteries::task::effective_token() {
        Some(t) => tokio::select! {
            biased;
            _ = t.cancelled() => {
                interrupt.interrupt();
                warn!(op, "cancelled by enclosing task");
                return Err(LuaError::external(format!(
                    "task cancelled during {op}"
                )));
            }
            r = wait => r,
        },
        None => wait.await,
    };

    let joined = match wait_result {
        Ok(j) => j,
        Err(d) => {
            interrupt.interrupt();
            warn!(op, timeout_ms = d.as_millis() as u64, "operation timeout");
            return Err(LuaError::external(format!(
                "{op} timeout ({}ms)",
                d.as_millis()
            )));
        }
    };

    joined
        .map_err(|e| {
            warn!(op, error = %e, "spawn_blocking join error");
            LuaError::external(format!("spawn_blocking: {e}"))
        })?
        .map_err(|e| {
            warn!(op, error = %e, "execution error");
            LuaError::external(e)
        })
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
        arr.set_metatable(Some(array_metatable(lua)?))?;
    }
    Ok(LuaValue::Table(arr))
}
