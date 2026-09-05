//! Async `std.time.sleep`.

use mlua::prelude::*;

/// Replace `time.sleep` with a cancel-aware async version.
///
/// Validation is [`crate::time::validate_sleep`] — the same finite /
/// non-negative / `max_sleep_secs` checks and the same messages the
/// blocking version raises.  The wait itself goes through
/// [`crate::task::race_sleep`], the primitive behind `std.task.sleep`, so
/// a cancel of the enclosing `task.scope` / `task.with_timeout` aborts it
/// with `"task cancelled"`.
pub(super) fn install(lua: &Lua, time_tbl: &LuaTable) -> LuaResult<()> {
    time_tbl.set(
        "sleep",
        lua.create_async_function(|lua: Lua, seconds: f64| async move {
            let dur = crate::time::validate_sleep(&lua, seconds)?;
            crate::task::race_sleep(dur).await
        })?,
    )
}
