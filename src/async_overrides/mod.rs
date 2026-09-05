//! Async replacements for the blocking entries of an already-registered
//! namespace.
//!
//! The core modules are synchronous by design: [`register_all`] gives you
//! a `std` table that needs no async runtime at all.  That is the right
//! default, but it means a script that calls `std.time.sleep(5)` or
//! `std.http.get(url)` inside a `std.task` scope parks the single VM
//! thread and every sibling task with it.
//!
//! This module is the opt-in fix.  Call [`register`] (or
//! [`register_by_name`]) *after* [`register_all`] and the blocking entries
//! are replaced in place by async ones:
//!
//! | Lua name | Behaviour after the override |
//! |---|---|
//! | `std.time.sleep` | `tokio::time::sleep`, cancelled by the enclosing scope like `std.task.sleep` |
//! | `std.proc.pipeline` | `run_pipeline` on the blocking pool |
//! | `std.http.get` / `.post` / `.request` | ureq call on the blocking pool |
//! | `std.fs.*` (all 14; 13 on non-unix, `symlink` is unix-only) | file I/O on the blocking pool |
//!
//! Lua-side names, arguments, return values and error messages are
//! unchanged — a script cannot tell the two apart except by no longer
//! starving its siblings.  Argument parsing, policy checks and result
//! table construction still happen on the VM thread; only the bulk
//! blocking work moves.  Policy resolution stays where it is, so the
//! `max_read_bytes` size stat and a `Sandboxed` path canonicalisation are
//! still short syscalls on the VM thread.  Both paths share the same helpers (`HttpCall`,
//! `walk_entries`, `run_pipeline`, `validate_sleep`, …), so the sync and
//! async behaviour cannot drift apart.
//!
//! # Runtime contract
//!
//! Identical to [`crate::task`]: a **tokio current-thread runtime driving
//! a `LocalSet`**.  The overridden functions are `create_async_function`s,
//! so they only run under `call_async` / `eval_async` — calling them from
//! a plain `Lua::load(...).eval()` raises the usual mlua "attempt to yield
//! from outside a coroutine" error.  `tokio::task::spawn_blocking` is
//! available on a current-thread runtime (the blocking pool is separate
//! from the core threads), so no multi-thread runtime is needed.
//!
//! `std.time.measure` calls its argument synchronously, so a function
//! passed to it must not call an overridden entry — that would yield
//! across the Rust call boundary and fail with the same mlua error.
//!
//! ```rust,ignore
//! // Requires the `task` feature.
//! let lua = Lua::new();
//! mlua_batteries::register_all(&lua, "std")?;
//! mlua_batteries::task::register(&lua)?;
//! mlua_batteries::async_overrides::register_by_name(&lua, "std")?;
//! // Run the VM inside `LocalSet::block_on` / `local.run_until(...)`.
//! ```
//!
//! # Cancellation
//!
//! `std.time.sleep` races the effective cancel token (see
//! [`crate::task::effective_token`]) exactly like `std.task.sleep`, and
//! raises `"task cancelled"` when the enclosing scope cancels.
//!
//! The `spawn_blocking` overrides (`proc`, `http`, `fs`) do **not**
//! interrupt work already handed to the blocking pool: a cancel of the
//! enclosing task cannot abort a running pipeline, HTTP request or file
//! read.  Each runs to completion — or, for `proc.pipeline`, to its own
//! `timeout_secs` — and the result is discarded if nobody is waiting for
//! it any more.  What the overrides do buy is that the VM thread stays
//! free while that happens, so siblings keep running.  Aborting a live
//! pipeline needs a native `tokio::process` implementation; that is left
//! for a later change.
//!
//! # Scope
//!
//! Only modules whose cargo feature is enabled *and* whose table is
//! present in the namespace are touched; anything else is left alone.
//! [`register`] is therefore safe to call on a partially-populated
//! namespace, and calling it twice is a no-op beyond replacing the same
//! entries again.
//!
//! [`register_all`]: crate::register_all

use mlua::prelude::*;

#[cfg(feature = "fs")]
mod fs;
#[cfg(feature = "http")]
mod http;
#[cfg(feature = "proc")]
mod proc;
#[cfg(feature = "time")]
mod time;

/// Replace the blocking entries of `ns` with async ones.
///
/// `ns` is the namespace table returned by [`crate::register_all`] /
/// [`crate::register_all_with`].  Each module is overridden only when its
/// cargo feature is enabled and its table is present in `ns`; a missing or
/// non-table entry is skipped without error.
///
/// # Errors
///
/// Propagates any Lua error raised while reading `ns` or while creating
/// the replacement functions.
#[cfg_attr(
    not(any(feature = "time", feature = "proc", feature = "http", feature = "fs")),
    allow(unused_variables)
)]
pub fn register(lua: &Lua, ns: &LuaTable) -> LuaResult<()> {
    macro_rules! override_module {
        ($name:literal, $mod:ident) => {{
            #[cfg(feature = $name)]
            if let LuaValue::Table(tbl) = ns.get::<LuaValue>($name)? {
                $mod::install(lua, &tbl)?;
            }
        }};
    }

    override_module!("time", time);
    override_module!("proc", proc);
    override_module!("http", http);
    override_module!("fs", fs);

    Ok(())
}

/// Look the namespace up in globals by name, then [`register`] into it.
///
/// Convenience for the common `register_by_name(&lua, "std")` call.
///
/// # Errors
///
/// Returns an error if `namespace` is absent from globals or is not a
/// table, plus anything [`register`] itself reports.
pub fn register_by_name(lua: &Lua, namespace: &str) -> LuaResult<()> {
    let ns: LuaTable = lua.globals().get(namespace).map_err(|e| {
        LuaError::external(format!(
            "async_overrides: namespace '{namespace}' is not a registered table: {e}"
        ))
    })?;
    register(lua, &ns)
}
