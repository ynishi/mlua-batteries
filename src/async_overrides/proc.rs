//! Async `std.proc.pipeline`.

use mlua::prelude::*;

/// Replace `proc.pipeline` with a version that runs the pipeline on the
/// blocking pool.
///
/// The Lua work stays on the VM thread: [`crate::proc::parse_pipeline_spec`]
/// reads the `(stages, opts)` tables, and
/// [`crate::proc::pipeline_result_to_lua`] builds the result table once the
/// run finishes.  Between the two, the owned [`crate::proc::PipelineSpec`]
/// moves onto `spawn_blocking` and drives the unchanged
/// [`crate::proc::run_pipeline`] — reader threads, poll loop and all.
///
/// # Known limitation
///
/// Cancelling the enclosing task does **not** abort a running pipeline.
/// Once `run_pipeline` is on the blocking pool it owns the child
/// processes, and nothing here can reach them; it returns when the
/// children exit or when its own `timeout_secs` fires and kills them.  A
/// cancelled caller therefore stops waiting, but the processes keep
/// going.  Fixing that means spawning through `tokio::process` so the
/// children hang off the runtime and can be killed on cancel — deliberately
/// out of scope here, since it would replace the runner rather than move it.
pub(super) fn install(lua: &Lua, proc_tbl: &LuaTable) -> LuaResult<()> {
    proc_tbl.set(
        "pipeline",
        lua.create_async_function(
            |lua: Lua, (stages, opts): (LuaTable, Option<LuaTable>)| async move {
                let spec =
                    crate::proc::parse_pipeline_spec(stages, opts).map_err(LuaError::external)?;

                let result = tokio::task::spawn_blocking(move || crate::proc::run_pipeline(&spec))
                    .await
                    .map_err(|e| {
                        LuaError::external(format!("std.proc.pipeline: spawn_blocking: {e}"))
                    })?
                    .map_err(LuaError::external)?;

                crate::proc::pipeline_result_to_lua(&lua, &result)
            },
        )?,
    )
}
