//! Smoke tests for `mlua_batteries::async_overrides`.
//!
//! Same runtime setup as `smoke_task.rs`: a current-thread runtime driving
//! a `LocalSet`, with scripts run through `eval_async`.  Each test that
//! touches a specific module carries that module's `cfg(feature = ...)`
//! guard, so the file still builds with `--features task` alone.
//!
//! The `fs` and `proc` tests run the *same script* against a plain
//! `register_all` VM and against an overridden one and compare the
//! results, which is what proves the override did not change behaviour.

#![cfg(feature = "task")]

use mlua::prelude::*;
use tokio::task::LocalSet;

/// A VM with `std` registered, `std.task` wired, and the async overrides
/// installed over the namespace.
fn overridden_lua() -> Lua {
    let lua = Lua::new();
    mlua_batteries::register_all(&lua, "std").unwrap();
    mlua_batteries::task::register(&lua).unwrap();
    mlua_batteries::async_overrides::register_by_name(&lua, "std").unwrap();
    lua
}

/// A VM with the stock blocking modules — the parity baseline.
///
/// Used only by the per-module tests, so it is dead code in a build that
/// enables `task` without any of the overridable modules.
#[allow(dead_code)]
fn plain_lua() -> Lua {
    let lua = Lua::new();
    mlua_batteries::register_all(&lua, "std").unwrap();
    lua
}

/// Run `body` on a current-thread runtime driving a `LocalSet`.
///
/// Dead code when none of the overridable modules is enabled.
#[allow(dead_code)]
fn block_on_local<F, T>(body: F) -> T
where
    F: std::future::Future<Output = T>,
{
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&rt, body)
}

/// The message line of a Lua error, without the stack traceback.
///
/// An async function is entered through mlua's poll shim, so its traceback
/// names `poll` where the blocking one names the called field.  That frame
/// is a property of the calling convention, not of the error, so parity
/// checks compare the message the two paths raise.
///
/// Dead code when neither `proc` nor `http` is enabled.
#[allow(dead_code)]
fn message_of(err: &LuaError) -> String {
    err.to_string()
        .split("stack traceback:")
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_string()
}

/// A fresh, uniquely-named directory under the platform temp dir.
///
/// Dead code when `fs` is not enabled.
#[allow(dead_code)]
fn temp_subdir(name: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "mlua_bat_async_ovr_{}_{}_{name}",
        std::process::id(),
        nanos
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ─── registration ────────────────────────────────────────────────────────

#[test]
fn overrides_keep_every_name_callable() {
    let lua = overridden_lua();

    lua.globals().set("__unix", cfg!(unix)).unwrap();
    let probe = lua
        .load(
            r#"
            local expected = {}
            if std.time then expected["time"] = { "sleep" } end
            if std.proc then expected["proc"] = { "pipeline" } end
            if std.http then expected["http"] = { "get", "post", "request" } end
            if std.fs then
                expected["fs"] = {
                    "read", "write", "read_binary", "write_binary",
                    "copy", "rename", "exists", "is_dir", "is_file",
                    "mkdir", "remove", "walk", "glob",
                }
                if __unix then table.insert(expected["fs"], "symlink") end
            end
            for mod_name, fns in pairs(expected) do
                for _, fn_name in ipairs(fns) do
                    assert(type(std[mod_name][fn_name]) == "function",
                           "std." .. mod_name .. "." .. fn_name .. " missing")
                end
            end
            return true
            "#,
        )
        .eval::<bool>();

    assert!(matches!(probe, Ok(true)), "probe failed: {probe:?}");
}

#[test]
fn register_by_name_rejects_unknown_namespace() {
    let lua = overridden_lua();
    let err = mlua_batteries::async_overrides::register_by_name(&lua, "nope").unwrap_err();
    assert!(
        err.to_string().contains("not a registered table"),
        "unexpected error: {err}"
    );
}

#[test]
fn register_skips_modules_absent_from_the_namespace() {
    // An empty namespace has no module tables at all; register must be a
    // no-op rather than an error.
    let lua = Lua::new();
    mlua_batteries::register_all(&lua, "std").unwrap();
    mlua_batteries::task::register(&lua).unwrap();
    let empty = lua.create_table().unwrap();
    mlua_batteries::async_overrides::register(&lua, &empty).expect("register on empty namespace");
    assert_eq!(empty.len().unwrap(), 0);
}

// ─── time ────────────────────────────────────────────────────────────────

#[cfg(feature = "time")]
#[test]
fn time_sleep_returns_and_actually_waits() {
    block_on_local(async {
        let lua = overridden_lua();
        let elapsed: f64 = lua
            .load(
                r#"
                local before = std.time.now()
                std.time.sleep(0.01)
                return std.time.now() - before
                "#,
            )
            .eval_async()
            .await
            .unwrap();
        assert!(elapsed >= 0.005, "slept for only {elapsed}s");
    });
}

#[cfg(feature = "time")]
#[test]
fn time_sleep_keeps_the_blocking_validation() {
    block_on_local(async {
        let lua = overridden_lua();
        for (script, expected) in [
            ("std.time.sleep(-1)", "finite non-negative"),
            ("std.time.sleep(0/0)", "finite non-negative"),
            ("std.time.sleep(86401)", "must not exceed"),
        ] {
            let err = lua
                .load(script)
                .eval_async::<LuaValue>()
                .await
                .expect_err("expected rejection");
            assert!(
                err.to_string().contains(expected),
                "script `{script}` gave: {err}"
            );
        }
    });
}

#[cfg(feature = "time")]
#[test]
fn time_sleep_is_cancelled_by_with_timeout() {
    block_on_local(async {
        let lua = overridden_lua();
        // The 10 ms deadline trips while the child's 5 s sleep is pending.
        // The override races the scope's cancel token exactly like
        // `std.task.sleep`, so the sleep itself raises "task cancelled" —
        // caught here by the child's own pcall — instead of running to term.
        let (msg, elapsed): (String, f64) = lua
            .load(
                r#"
                local inner = "<never ran>"
                local before = std.time.now()
                pcall(function()
                    std.task.with_timeout(10, function(scope)
                        scope:spawn(function()
                            local ok, err = pcall(function()
                                std.time.sleep(5)
                            end)
                            inner = tostring(err)
                        end)
                        std.time.sleep(5)
                    end)
                end)
                return inner, std.time.now() - before
                "#,
            )
            .eval_async()
            .await
            .unwrap();
        assert!(
            msg.contains("task cancelled"),
            "sleep was not cancelled: {msg}"
        );
        assert!(elapsed < 1.0, "cancellation took {elapsed}s");
    });
}

#[cfg(feature = "time")]
#[test]
fn time_sleep_does_not_block_sibling_tasks() {
    block_on_local(async {
        let lua = overridden_lua();
        // Two 150 ms sleeps in sibling tasks. If `time.sleep` still parked
        // the VM thread they would serialise to >= 300 ms; the 250 ms bound
        // leaves 100 ms of scheduling slack for a loaded machine.
        let elapsed: f64 = lua
            .load(
                r#"
                local before = std.time.now()
                std.task.scope(function(scope)
                    scope:spawn(function() std.time.sleep(0.15) end)
                    scope:spawn(function() std.time.sleep(0.15) end)
                end)
                return std.time.now() - before
                "#,
            )
            .eval_async()
            .await
            .unwrap();
        assert!(
            elapsed < 0.25,
            "sleeps serialised ({elapsed}s) — still blocking the VM thread"
        );
    });
}

// ─── fs ──────────────────────────────────────────────────────────────────

#[cfg(feature = "fs")]
const FS_ROUND_TRIP: &str = r#"
    local dir = ...
    local out = {}
    std.fs.mkdir(dir .. "/nested")
    std.fs.write(dir .. "/a.txt", "alpha")
    std.fs.write(dir .. "/b.log", "beta")
    std.fs.write(dir .. "/nested/c.txt", "gamma")
    out.read = std.fs.read(dir .. "/a.txt")
    out.exists_before = std.fs.exists(dir .. "/b.log")
    out.is_dir = std.fs.is_dir(dir .. "/nested")
    out.is_file = std.fs.is_file(dir .. "/a.txt")

    std.fs.write_binary(dir .. "/bin.dat", "\1\2\3")
    out.binary = std.fs.read_binary(dir .. "/bin.dat")

    std.fs.copy(dir .. "/a.txt", dir .. "/a-copy.txt")
    out.copied = std.fs.read(dir .. "/a-copy.txt")
    std.fs.rename(dir .. "/a-copy.txt", dir .. "/a-moved.txt")
    out.renamed = std.fs.read(dir .. "/a-moved.txt")

    -- Strip the directory prefix per entry with a plain `sub`: `dir` comes
    -- from the host's temp dir and may hold Lua pattern characters.
    local function relative(paths)
        local rel = {}
        for i, p in ipairs(paths) do rel[i] = p:sub(#dir + 2) end
        table.sort(rel)
        return table.concat(rel, "|")
    end
    out.walk = relative(std.fs.walk(dir))
    out.glob = relative(std.fs.glob(dir .. "/*.txt"))

    std.fs.remove(dir .. "/b.log")
    out.exists_after = std.fs.exists(dir .. "/b.log")
    return out
"#;

/// Read the fields the round-trip script fills in, as one comparable tuple.
#[cfg(feature = "fs")]
fn fs_summary(
    t: &LuaTable,
) -> (
    String,
    bool,
    bool,
    bool,
    String,
    String,
    String,
    String,
    String,
    bool,
) {
    (
        t.get("read").unwrap(),
        t.get("exists_before").unwrap(),
        t.get("is_dir").unwrap(),
        t.get("is_file").unwrap(),
        t.get("binary").unwrap(),
        t.get("copied").unwrap(),
        t.get("renamed").unwrap(),
        t.get("walk").unwrap(),
        t.get("glob").unwrap(),
        t.get("exists_after").unwrap(),
    )
}

#[cfg(feature = "fs")]
#[test]
fn fs_round_trip_matches_the_blocking_module() {
    let async_dir = temp_subdir("fs_async");
    let sync_dir = temp_subdir("fs_sync");

    let async_result = block_on_local(async {
        let lua = overridden_lua();
        let t: LuaTable = lua
            .load(FS_ROUND_TRIP)
            .call_async(async_dir.to_string_lossy().to_string())
            .await
            .unwrap();
        fs_summary(&t)
    });

    let sync_result = {
        let lua = plain_lua();
        let t: LuaTable = lua
            .load(FS_ROUND_TRIP)
            .call(sync_dir.to_string_lossy().to_string())
            .unwrap();
        fs_summary(&t)
    };

    assert_eq!(async_result.0, "alpha");
    assert!(async_result.1, "b.log should exist before remove");
    assert!(async_result.2, "nested should be a dir");
    assert!(async_result.3, "a.txt should be a file");
    assert_eq!(async_result.4, "\u{1}\u{2}\u{3}");
    assert_eq!(async_result.5, "alpha");
    assert_eq!(async_result.6, "alpha");
    assert!(
        async_result.8.contains("a.txt") && !async_result.8.contains("b.log"),
        "glob entries: {}",
        async_result.8
    );
    assert!(!async_result.9, "b.log should be gone after remove");

    assert_eq!(
        async_result, sync_result,
        "async overrides diverged from the blocking module"
    );

    let _ = std::fs::remove_dir_all(&async_dir);
    let _ = std::fs::remove_dir_all(&sync_dir);
}

#[cfg(feature = "fs")]
#[test]
fn fs_walk_and_glob_return_the_expected_entries() {
    let dir = temp_subdir("fs_walk_glob");

    block_on_local(async {
        let lua = overridden_lua();
        let (walk, glob): (String, String) = lua
            .load(
                r#"
                local dir = ...
                std.fs.mkdir(dir .. "/sub")
                std.fs.write(dir .. "/one.txt", "1")
                std.fs.write(dir .. "/two.md", "2")
                std.fs.write(dir .. "/sub/three.txt", "3")

                local walked = std.fs.walk(dir)
                table.sort(walked)
                local globbed = std.fs.glob(dir .. "/*.txt")
                table.sort(globbed)
                return table.concat(walked, "|"), table.concat(globbed, "|")
                "#,
            )
            .call_async::<(String, String)>(dir.to_string_lossy().to_string())
            .await
            .unwrap();

        let base = dir.to_string_lossy().to_string();
        assert_eq!(
            walk,
            format!("{base}/one.txt|{base}/sub/three.txt|{base}/two.md")
        );
        // `*.txt` uses a literal separator, so `sub/three.txt` is excluded.
        assert_eq!(glob, format!("{base}/one.txt"));
    });

    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "fs")]
#[test]
fn fs_read_still_enforces_max_read_bytes() {
    let dir = temp_subdir("fs_max_read");
    let file = dir.join("big.txt");
    std::fs::write(&file, "0123456789").unwrap();

    block_on_local(async {
        let lua = Lua::new();
        let config = mlua_batteries::config::Config::builder()
            .max_read_bytes(4)
            .build()
            .unwrap();
        mlua_batteries::register_all_with(&lua, "std", config).unwrap();
        mlua_batteries::task::register(&lua).unwrap();
        mlua_batteries::async_overrides::register_by_name(&lua, "std").unwrap();

        let err = lua
            .load(r#"return std.fs.read(...)"#)
            .call_async::<LuaValue>(file.to_string_lossy().to_string())
            .await
            .expect_err("expected the size limit to reject the read");
        assert!(
            err.to_string().contains("exceeds max_read_bytes limit"),
            "unexpected error: {err}"
        );
    });

    let _ = std::fs::remove_dir_all(&dir);
}

// ─── proc ────────────────────────────────────────────────────────────────

#[cfg(feature = "proc")]
const PROC_PIPELINE: &str = r#"
    local r = std.proc.pipeline({
        { argv = { "echo", "hello pipeline" } },
    })
    return r.ok, r.timed_out, #r.stages, r.stages[1].exit_code, r.stages[1].stdout
"#;

#[cfg(feature = "proc")]
#[test]
fn proc_pipeline_matches_the_blocking_module() {
    let async_result = block_on_local(async {
        let lua = overridden_lua();
        lua.load(PROC_PIPELINE)
            .eval_async::<(bool, bool, usize, i32, String)>()
            .await
            .unwrap()
    });

    let sync_result = plain_lua()
        .load(PROC_PIPELINE)
        .eval::<(bool, bool, usize, i32, String)>()
        .unwrap();

    assert!(async_result.0, "pipeline should have succeeded");
    assert!(!async_result.1, "pipeline should not have timed out");
    assert_eq!(async_result.2, 1);
    assert_eq!(async_result.3, 0);
    assert_eq!(async_result.4, "hello pipeline\n");
    assert_eq!(
        async_result, sync_result,
        "async pipeline diverged from the blocking module"
    );
}

#[cfg(feature = "proc")]
#[test]
fn proc_pipeline_reports_setup_errors_like_the_blocking_module() {
    let script = r#"return std.proc.pipeline({ { argv = {} } })"#;

    let async_err = block_on_local(async {
        message_of(
            &overridden_lua()
                .load(script)
                .eval_async::<LuaValue>()
                .await
                .expect_err("empty argv must be rejected"),
        )
    });
    let sync_err = message_of(
        &plain_lua()
            .load(script)
            .eval::<LuaValue>()
            .expect_err("empty argv must be rejected"),
    );

    assert!(
        async_err.contains("argv must be non-empty"),
        "unexpected error: {async_err}"
    );
    assert_eq!(async_err, sync_err);
}

// ─── http (no network) ───────────────────────────────────────────────────

#[cfg(feature = "http")]
#[test]
fn http_overrides_reject_bad_input_like_the_blocking_module() {
    let cases = [
        (
            r#"return std.http.request({ method = "TRACE", url = "http://localhost:0/nope" })"#,
            "unsupported HTTP method",
        ),
        (
            r#"return std.http.request({ method = "POST", url = "http://localhost:0/nope", body = 12345 })"#,
            "body must be a string",
        ),
        (
            r#"return std.http.request({ url = "http://localhost:0/nope" })"#,
            "",
        ),
    ];

    for (script, expected) in cases {
        let async_err = block_on_local(async {
            message_of(
                &overridden_lua()
                    .load(script)
                    .eval_async::<LuaValue>()
                    .await
                    .expect_err("expected rejection"),
            )
        });
        let sync_err = message_of(
            &plain_lua()
                .load(script)
                .eval::<LuaValue>()
                .expect_err("expected rejection"),
        );

        assert!(
            async_err.contains(expected),
            "script `{script}` gave: {async_err}"
        );
        assert_eq!(async_err, sync_err, "diverged for `{script}`");
    }
}

/// A denied URL must fail during the policy check on the VM thread — the
/// request never reaches the network, so this test needs none.
#[cfg(all(feature = "http", feature = "fs"))]
#[test]
fn http_policy_rejection_matches_the_blocking_module() {
    fn deny_all_config() -> mlua_batteries::config::Config {
        mlua_batteries::config::Config::builder()
            .http_policy(mlua_batteries::policy::HttpAllowList::new(["example.com"]))
            .build()
            .unwrap()
    }
    let script = r#"return std.http.get("http://blocked.invalid/x")"#;

    let async_err = block_on_local(async {
        let lua = Lua::new();
        mlua_batteries::register_all_with(&lua, "std", deny_all_config()).unwrap();
        mlua_batteries::task::register(&lua).unwrap();
        mlua_batteries::async_overrides::register_by_name(&lua, "std").unwrap();
        message_of(
            &lua.load(script)
                .eval_async::<LuaValue>()
                .await
                .expect_err("policy must reject the host"),
        )
    });

    let lua = Lua::new();
    mlua_batteries::register_all_with(&lua, "std", deny_all_config()).unwrap();
    let sync_err = message_of(
        &lua.load(script)
            .eval::<LuaValue>()
            .expect_err("policy must reject the host"),
    );

    assert!(
        async_err.contains("does not match any allowed host"),
        "unexpected error: {async_err}"
    );
    assert_eq!(async_err, sync_err);
}
