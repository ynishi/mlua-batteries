//! Smoke tests for the `std.sql` / `std.kv` bridges.
//!
//! Intentionally minimal — full end-to-end coverage lives in the host crate.
//! Here we only verify that:
//!   - `register*` succeeds and creates the expected `std.*` tables
//!   - the basic happy-path round trip works for each bridge
//!   - the cross-bridge wiring (sql cancellation under task) compiles and links

use std::sync::{Arc, Mutex};

use mlua::prelude::*;
use tokio::task::LocalSet;

// This crate's `rusqlite`, reached through the re-export so the test does not
// need a dependency of its own that could drift onto another cluster.
use mlua_batteries_sqlite::rusqlite::{Connection, InterruptHandle};

fn open_in_memory_pair() -> (Arc<Mutex<Connection>>, Arc<InterruptHandle>) {
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

/// Like [`make_lua`], but with the regular `std.*` modules registered so the
/// test can use `std.json.encode` / `std.json.array` to inspect what the
/// sql / kv bridges store and hand back.
fn make_lua_with_std_modules() -> Lua {
    let lua = Lua::new();
    mlua_batteries::register_all(&lua, "std").expect("register_all");
    lua
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
        mlua_batteries_sqlite::sql::register(&lua, conn, interrupt).unwrap();

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
        mlua_batteries_sqlite::sql::register(&lua, conn, interrupt).unwrap();

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
fn sql_zero_row_result_encodes_as_array() {
    // A query returning no rows is an empty *list*, not an empty object.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = LocalSet::new();

    local.block_on(&rt, async {
        let lua = make_lua_with_std_modules();
        mlua_batteries::task::register(&lua).unwrap();
        let (conn, interrupt) = open_in_memory_pair();
        mlua_batteries_sqlite::sql::register(&lua, conn, interrupt).unwrap();

        let ok: bool = lua
            .load(
                r#"
                std.sql.exec("CREATE TABLE t(x INTEGER)")

                local empty = std.sql.query("SELECT x FROM t")
                assert(#empty == 0, "expected zero rows")
                local encoded = std.json.encode(empty)
                assert(encoded == "[]", "zero-row result: " .. encoded)

                std.sql.exec("INSERT INTO t(x) VALUES(1)")
                local one = std.sql.query("SELECT x FROM t")
                assert(#one == 1 and getmetatable(one) == nil, "non-empty result untagged")
                assert(std.json.encode(one) == '[{"x":1}]', "non-empty result unchanged")
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
fn kv_empty_list_encodes_as_array() {
    // Same for `kv.list`: an empty namespace — or a prefix that matches
    // nothing — must encode as `[]`.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = LocalSet::new();

    local.block_on(&rt, async {
        let lua = make_lua_with_std_modules();
        mlua_batteries::task::register(&lua).unwrap();
        let (conn, interrupt) = open_in_memory_pair();
        mlua_batteries_sqlite::kv::register(&lua, conn, interrupt).unwrap();

        let ok: bool = lua
            .load(
                r#"
                local empty = std.json.encode(std.kv.list("emptyns"))
                assert(empty == "[]", "empty namespace: " .. empty)

                std.kv.set("ns1", "alpha", 1)
                local no_match = std.json.encode(std.kv.list("ns1", "zzz"))
                assert(no_match == "[]", "no prefix match: " .. no_match)

                local hit = std.kv.list("ns1", "al")
                assert(#hit == 1 and getmetatable(hit) == nil, "non-empty list untagged")
                assert(std.json.encode(hit) == '["alpha"]', "non-empty list unchanged")
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
        mlua_batteries_sqlite::kv::register(&lua, conn, interrupt).unwrap();

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
fn kv_preserves_empty_array_round_trip() {
    // `std.kv` serializes through the NULL-preserving converters in
    // `crate::sql`.  An empty array must survive set → get → re-encode as
    // `[]`, not degrade into `{}`.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = LocalSet::new();

    local.block_on(&rt, async {
        let lua = make_lua_with_std_modules();
        mlua_batteries::task::register(&lua).unwrap();
        let (conn, interrupt) = open_in_memory_pair();
        mlua_batteries_sqlite::kv::register(&lua, conn, interrupt).unwrap();

        let ok: bool = lua
            .load(
                r#"
                std.kv.set("ns1", "top", std.json.array())
                std.kv.set("ns1", "nested", {items = std.json.array(), name = "x"})

                local top = std.json.encode(std.kv.get("ns1", "top"))
                assert(top == "[]", "top-level empty array: " .. top)

                local nested = std.json.encode(std.kv.get("ns1", "nested"))
                assert(nested == '{"items":[],"name":"x"}', "nested: " .. nested)

                -- The tag metatable is the one `std.json` hands out, so both
                -- bridges agree on the convention.
                assert(getmetatable(std.kv.get("ns1", "top"))
                       == getmetatable(std.json.array()), "metatable identity")
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
fn kv_empty_table_still_round_trips_as_object() {
    // Guardrail for the change above: an *untagged* empty table keeps its
    // previous meaning (JSON object).
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = LocalSet::new();

    local.block_on(&rt, async {
        let lua = make_lua_with_std_modules();
        mlua_batteries::task::register(&lua).unwrap();
        let (conn, interrupt) = open_in_memory_pair();
        mlua_batteries_sqlite::kv::register(&lua, conn, interrupt).unwrap();

        let ok: bool = lua
            .load(
                r#"
                std.kv.set("ns1", "obj", {})
                local encoded = std.json.encode(std.kv.get("ns1", "obj"))
                assert(encoded == "{}", "untagged empty table: " .. encoded)

                std.kv.set("ns1", "list", {1, 2, 3})
                local list = std.kv.get("ns1", "list")
                assert(#list == 3 and getmetatable(list) == nil, "non-empty array untouched")
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
fn kv_null_sentinel_and_empty_array_coexist() {
    // The empty-array branch was added to the NULL-preserving converters —
    // verify the NULL sentinel still round-trips alongside it.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = LocalSet::new();

    local.block_on(&rt, async {
        let lua = make_lua_with_std_modules();
        mlua_batteries::task::register(&lua).unwrap();
        let (conn, interrupt) = open_in_memory_pair();
        mlua_batteries_sqlite::sql::register(&lua, conn.clone(), interrupt.clone()).unwrap();
        mlua_batteries_sqlite::kv::register(&lua, conn, interrupt).unwrap();

        let ok: bool = lua
            .load(
                r#"
                std.kv.set("ns1", "mix", {flag = std.sql.null, items = std.json.array()})
                local v = std.kv.get("ns1", "mix")
                assert(v.flag == std.sql.null, "NULL sentinel lost")
                local encoded = std.json.encode(v)
                assert(encoded == '{"flag":null,"items":[]}', "mixed value: " .. encoded)
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
        mlua_batteries_sqlite::kv::register(&lua, conn, interrupt).unwrap();

        let err = lua
            .load(r#"std.kv.set("bad/ns", "k", "v")"#)
            .eval_async::<LuaValue>()
            .await;
        assert!(err.is_err(), "expected error for invalid namespace");
    });
}
