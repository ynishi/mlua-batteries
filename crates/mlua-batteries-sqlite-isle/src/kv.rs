//! `std.kv` — SQLite-backed key-value store for Lua scripts.
//!
//! Storage lives in a SQLite database supplied by the host (one shared
//! connection), in a dedicated `__kv` table:
//!
//! ```sql
//! CREATE TABLE __kv (
//!     ns    TEXT NOT NULL,
//!     key   TEXT NOT NULL,
//!     value TEXT NOT NULL,   -- JSON-serialized Lua value
//!     PRIMARY KEY (ns, key)
//! ) WITHOUT ROWID;
//! ```
//!
//! Trade-offs vs. a JSON-file-per-namespace implementation:
//! - Per-key updates (no whole-namespace rewrite on every set).
//! - Durability + atomicity delegated to SQLite's WAL journal.
//! - Cross-process writes arbitrated by `busy_timeout`.
//!
//! # Wiring contract
//!
//! Symmetric with [`crate::sql`].  The host opens an `AsyncIsle` (typically
//! over a database file dedicated to KV scratch state, kept separate from the
//! `std.sql` user database so backup / WAL / page-cache lifecycles do not
//! collide) and passes a clone of the handle.  Cancellation and per-query
//! timeout are inherited from the [`crate::sql::SqlConfig`] in
//! `lua.app_data`; the `rusqlite` / `rusqlite-isle` versions come from this
//! release line (see [the crate docs](crate)).
//!
//! Registration is `async` because the `__kv` table is created through the
//! isle before the module is exposed to Lua.  Hosts that would rather do it
//! at open time can call [`init_schema`] from the isle's `init` closure —
//! the DDL is `CREATE TABLE IF NOT EXISTS`, so running it twice is harmless.
//!
//! Writes (`set` / `delete`) run inside a `BEGIN IMMEDIATE` transaction, so
//! they take the write lock up front rather than risking the
//! timeout-bypassing deferred upgrade under cross-process contention.

use mlua::prelude::*;

use mlua_batteries::json::{
    array_metatable, json_to_lua_preserving_null, lua_to_json_preserving_null,
};

use crate::sql::{run_job, sql_query_timeout, SqlConfig};
use crate::sqlite_backend::rusqlite::{self, Connection, OptionalExtension};
use crate::sqlite_backend::rusqlite_isle::AsyncIsle;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Validate a namespace string.
///
/// Namespaces were originally used as filenames, so `/`, `\`, `..`, `\0` were
/// rejected for path-traversal safety.  Even though storage is now a SQL table
/// (and namespaces are just column values), we keep the same validation so
/// that existing Lua scripts and tests see identical semantics.
fn validate_ns(ns: &str) -> Result<(), String> {
    if ns.is_empty() {
        return Err(format!("Invalid namespace: '{ns}'"));
    }
    if ns.contains('/') || ns.contains('\\') || ns.contains('\0') || ns.contains("..") {
        return Err(format!("Invalid namespace: '{ns}'"));
    }
    Ok(())
}

/// Create the `__kv` table if it is not there yet.
///
/// [`register`] / [`register_with`] run this through the isle before exposing
/// `std.kv`, so hosts do not have to.  It is public for hosts that prefer to
/// do it at open time, from the isle's `init` closure:
///
/// ```rust,ignore
/// let (isle, driver) = AsyncIsle::spawn("kv.db", mlua_batteries::kv::init_schema).await?;
/// ```
pub fn init_schema(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS __kv (\n                ns    TEXT NOT NULL,\n                key   TEXT NOT NULL,\n                value TEXT NOT NULL,\n                PRIMARY KEY (ns, key)\n            ) WITHOUT ROWID;",
    )
}

/// Run one write statement under `BEGIN IMMEDIATE`, returning the row count.
fn kv_write(
    conn: &mut Connection,
    op: &'static str,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<usize, String> {
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| format!("{op} begin: {e}"))?;
    let affected = tx
        .execute(sql, params)
        .map_err(|e| format!("{op} sql error: {e}"))?;
    tx.commit().map_err(|e| format!("{op} commit: {e}"))?;
    Ok(affected)
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register `std.kv` with default [`SqlConfig`] (only used if `std.sql` was
/// not registered first; otherwise the existing config is preserved).
pub async fn register(lua: &Lua, isle: AsyncIsle) -> LuaResult<()> {
    register_with(lua, isle, SqlConfig::default()).await
}

/// Register `std.kv` with caller-provided [`SqlConfig`].
///
/// If `std.sql` was registered earlier with a `SqlConfig`, the same slot
/// in `lua.app_data` is overwritten — pass an identical config from the
/// host to keep `sql` and `kv` in sync (the typical case).
pub async fn register_with(lua: &Lua, isle: AsyncIsle, cfg: SqlConfig) -> LuaResult<()> {
    lua.set_app_data::<SqlConfig>(cfg);

    // One-time schema init, through the isle that owns the connection.
    isle.call(init_schema)
        .await
        .map_err(|e| LuaError::external(format!("kv schema init: {e}")))?;

    let kv_tbl = lua.create_table()?;

    // ── std.kv.get ────────────────────────────────────────────────────────
    {
        let isle = isle.clone();
        kv_tbl.set(
            "get",
            lua.create_async_function(move |lua, (ns, key): (String, String)| {
                let isle = isle.clone();
                let ns_check = validate_ns(&ns).map_err(LuaError::external);
                async move {
                    ns_check?;
                    let timeout = sql_query_timeout(&lua);
                    let row = run_job(&isle, timeout, "kv.get", move |conn| {
                        Ok(conn
                            .query_row(
                                "SELECT value FROM __kv WHERE ns = ?1 AND key = ?2",
                                rusqlite::params![ns, key],
                                |row| row.get::<_, String>(0),
                            )
                            .optional()
                            .map_err(|e| format!("kv.get sql error: {e}")))
                    })
                    .await?;
                    match row {
                        None => Ok(LuaValue::Nil),
                        Some(s) => {
                            let v: serde_json::Value = serde_json::from_str(&s).map_err(|e| {
                                LuaError::external(format!("kv.get json parse: {e}"))
                            })?;
                            json_to_lua_preserving_null(&lua, v)
                        }
                    }
                }
            })?,
        )?;
    }

    // ── std.kv.set ────────────────────────────────────────────────────────
    {
        let isle = isle.clone();
        kv_tbl.set(
            "set",
            lua.create_async_function(move |lua, (ns, key, value): (String, String, LuaValue)| {
                let isle = isle.clone();
                // Serialize synchronously on the Lua thread (LuaValue is
                // !Send, so it can't cross the job boundary).
                let ns_check = validate_ns(&ns).map_err(LuaError::external);
                let json_result = lua_to_json_preserving_null(value).and_then(|v| {
                    serde_json::to_string(&v)
                        .map_err(|e| LuaError::external(format!("kv.set serialize: {e}")))
                });
                async move {
                    ns_check?;
                    let json_str = json_result?;
                    let timeout = sql_query_timeout(&lua);
                    run_job(&isle, timeout, "kv.set", move |conn| {
                        Ok(kv_write(
                            conn,
                            "kv.set",
                            "INSERT INTO __kv (ns, key, value) VALUES (?1, ?2, ?3) \
                                 ON CONFLICT(ns, key) DO UPDATE SET value = excluded.value",
                            &[&ns, &key, &json_str],
                        )
                        .map(|_| ()))
                    })
                    .await
                }
            })?,
        )?;
    }

    // ── std.kv.delete ─────────────────────────────────────────────────────
    {
        let isle = isle.clone();
        kv_tbl.set(
            "delete",
            lua.create_async_function(move |lua, (ns, key): (String, String)| {
                let isle = isle.clone();
                let ns_check = validate_ns(&ns).map_err(LuaError::external);
                async move {
                    ns_check?;
                    let timeout = sql_query_timeout(&lua);
                    run_job(&isle, timeout, "kv.delete", move |conn| {
                        Ok(kv_write(
                            conn,
                            "kv.delete",
                            "DELETE FROM __kv WHERE ns = ?1 AND key = ?2",
                            &[&ns, &key],
                        )
                        .map(|n| n > 0))
                    })
                    .await
                }
            })?,
        )?;
    }

    // ── std.kv.list ───────────────────────────────────────────────────────
    {
        let isle = isle.clone();
        kv_tbl.set(
            "list",
            lua.create_async_function(move |lua, (ns, prefix): (String, Option<String>)| {
                let isle = isle.clone();
                let ns_check = validate_ns(&ns).map_err(LuaError::external);
                async move {
                    ns_check?;
                    let timeout = sql_query_timeout(&lua);
                    let keys = run_job(&isle, timeout, "kv.list", move |conn| {
                        Ok((|| {
                            let mut stmt = conn
                                .prepare("SELECT key FROM __kv WHERE ns = ?1 ORDER BY key")
                                .map_err(|e| format!("kv.list prepare: {e}"))?;
                            let keys: Vec<String> = stmt
                                .query_map(rusqlite::params![ns], |row| row.get::<_, String>(0))
                                .map_err(|e| format!("kv.list query: {e}"))?
                                .collect::<Result<_, _>>()
                                .map_err(|e| format!("kv.list row: {e}"))?;
                            Ok::<_, String>(keys)
                        })())
                    })
                    .await?;

                    let tbl = lua.create_table()?;
                    let mut idx = 1usize;
                    for k in keys {
                        let include = prefix.as_deref().map_or(true, |p| k.starts_with(p));
                        if include {
                            tbl.set(idx, k.as_str())?;
                            idx += 1;
                        }
                    }
                    if idx == 1 {
                        // Nothing matched: tag the empty list with the shared
                        // `__jsontype = "array"` metatable so that
                        // `json.encode(kv.list(...))` renders `[]`, not `{}`.
                        tbl.set_metatable(Some(array_metatable(&lua)?))?;
                    }
                    Ok(LuaValue::Table(tbl))
                }
            })?,
        )?;
    }

    let std_ns: LuaTable = lua.globals().get("std")?;
    std_ns.set("kv", kv_tbl)?;
    Ok(())
}
