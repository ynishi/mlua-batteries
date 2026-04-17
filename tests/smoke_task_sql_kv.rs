//! Smoke tests for the `task` / `sql` / `kv` features.
//!
//! These are intentionally minimal — full end-to-end coverage lives in
//! the host crate (`agent-block`).  Here we only verify that:
//!   - `register*` succeeds and creates the expected `std.*` tables
//!   - the basic happy-path round trip works for each bridge
//!   - the cross-bridge wiring (sql cancellation under task) compiles
//!     and links

#![cfg(all(feature = "task", feature = "sql", feature = "kv"))]

use std::sync::{Arc, Mutex};

use mlua::prelude::*;
use rusqlite::Connection;
use tokio::task::LocalSet;

fn open_in_memory_pair() -> (Arc<Mutex<Connection>>, Arc<rusqlite::InterruptHandle>) {
    let conn = Connection::open_in_memory().expect("open :memory:");
    let interrupt = Arc::new(conn.get_interrupt_handle());
    (Arc::new(Mutex::new(conn)), interrupt)
}

fn make_lua() -> Lua {
    let lua = Lua::new();
    let std = lua.create_table().unwrap();
    lua.globals().set("std", std).unwrap();
    lua
}

#[test]
fn task_register_creates_std_task_table() {
    let lua = make_lua();
    mlua_batteries::task::register(&lua).expect("task::register");

    // Verify the table exists and the expected callables are present.
    let probe = lua
        .load(
            r#"
            assert(type(std.task) == "table", "std.task missing")
            for _, fn_name in ipairs({
                "spawn", "sleep", "yield", "checkpoint",
                "cancel_token", "current", "scope", "with_timeout",
            }) do
                assert(type(std.task[fn_name]) == "function",
                       "std.task." .. fn_name .. " missing")
            end
            return true
            "#,
        )
        .eval::<bool>();

    assert!(matches!(probe, Ok(true)), "probe failed: {probe:?}");
}

#[test]
fn task_sleep_and_current_inside_localset() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = LocalSet::new();

    local.block_on(&rt, async {
        let lua = make_lua();
        mlua_batteries::task::register(&lua).unwrap();

        // Sleep for 1 ms, then verify std.task.current() returns nil at the
        // top level (we are not inside a spawned task here).
        let outside = lua
            .load(
                r#"
                std.task.sleep(1)
                return std.task.current()
                "#,
            )
            .eval_async::<LuaValue>()
            .await
            .unwrap();
        assert!(matches!(outside, LuaValue::Nil));

        // Inside a spawned task, current() must return a non-nil table.
        let inside_id: String = lua
            .load(
                r#"
                local h = std.task.spawn(function()
                    return std.task.current().id
                end)
                return h:join()
                "#,
            )
            .eval_async()
            .await
            .unwrap();
        assert!(inside_id.starts_with('t'), "unexpected id: {inside_id}");
    });
}

#[test]
fn sql_query_and_exec_round_trip() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = LocalSet::new();

    local.block_on(&rt, async {
        let lua = make_lua();
        mlua_batteries::task::register(&lua).unwrap();
        let (conn, interrupt) = open_in_memory_pair();
        mlua_batteries::sql::register(&lua, conn, interrupt).unwrap();

        let result: i64 = lua
            .load(
                r#"
                local r1 = std.sql.exec("CREATE TABLE t(x INTEGER, y TEXT)")
                local r2 = std.sql.exec("INSERT INTO t(x, y) VALUES(?, ?)", {42, "hello"})
                assert(r2.affected == 1, "affected mismatch")
                local rows = std.sql.query("SELECT x, y FROM t WHERE x = ?", {42})
                assert(#rows == 1, "row count")
                assert(rows[1].y == "hello", "y col")
                return rows[1].x
                "#,
            )
            .eval_async()
            .await
            .unwrap();
        assert_eq!(result, 42);
    });
}

#[test]
fn sql_null_sentinel_round_trip() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = LocalSet::new();

    local.block_on(&rt, async {
        let lua = make_lua();
        mlua_batteries::task::register(&lua).unwrap();
        let (conn, interrupt) = open_in_memory_pair();
        mlua_batteries::sql::register(&lua, conn, interrupt).unwrap();

        let is_null: bool = lua
            .load(
                r#"
                std.sql.exec("CREATE TABLE n(v INTEGER)")
                std.sql.exec("INSERT INTO n(v) VALUES(NULL)")
                local rows = std.sql.query("SELECT v FROM n")
                return rows[1].v == std.sql.null
                "#,
            )
            .eval_async()
            .await
            .unwrap();
        assert!(is_null, "NULL did not round-trip via std.sql.null sentinel");
    });
}

#[test]
fn kv_set_get_list_delete() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = LocalSet::new();

    local.block_on(&rt, async {
        let lua = make_lua();
        mlua_batteries::task::register(&lua).unwrap();
        let (conn, interrupt) = open_in_memory_pair();
        mlua_batteries::kv::register(&lua, conn, interrupt).unwrap();

        let ok: bool = lua
            .load(
                r#"
                std.kv.set("ns1", "a", "alpha")
                std.kv.set("ns1", "b", {nested = true, n = 7})
                assert(std.kv.get("ns1", "a") == "alpha", "get a")
                local b = std.kv.get("ns1", "b")
                assert(b.nested == true and b.n == 7, "get b nested")
                local keys = std.kv.list("ns1")
                assert(#keys == 2 and keys[1] == "a" and keys[2] == "b", "list")
                local removed = std.kv.delete("ns1", "a")
                assert(removed == true, "delete returns true")
                assert(std.kv.get("ns1", "a") == nil, "deleted a is nil")
                return true
                "#,
            )
            .eval_async()
            .await
            .unwrap();
        assert!(ok);
    });
}

#[test]
fn kv_rejects_invalid_namespace() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = LocalSet::new();

    local.block_on(&rt, async {
        let lua = make_lua();
        mlua_batteries::task::register(&lua).unwrap();
        let (conn, interrupt) = open_in_memory_pair();
        mlua_batteries::kv::register(&lua, conn, interrupt).unwrap();

        let err = lua
            .load(r#"std.kv.set("bad/ns", "k", "v")"#)
            .eval_async::<LuaValue>()
            .await;
        assert!(err.is_err(), "expected error for invalid namespace");
    });
}
