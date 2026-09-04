//! Smoke tests for the `task` feature.
//!
//! Intentionally minimal — the `std.sql` / `std.kv` bridges moved to the
//! `mlua-batteries-sqlite` crate in 0.5.0 and carry their own smoke tests.

#![cfg(feature = "task")]

use mlua::prelude::*;
use tokio::task::LocalSet;

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
