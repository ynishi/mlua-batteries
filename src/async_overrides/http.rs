//! Async `std.http.get` / `.post` / `.request`.

use mlua::prelude::*;

use crate::http::{build_response, prepare_get, prepare_post, prepare_request, HttpCall};

/// Send an already-resolved call on the blocking pool.
///
/// [`HttpCall`] carries the cloned [`ureq::Agent`] (which is `Clone + Send`
/// in ureq 3, sharing the connection pool) plus owned strings, and
/// [`HttpCall::run`] reports failures as a plain `String`, so both the
/// value and its error cross the `spawn_blocking` boundary.  The message
/// is the one `e.to_string()` produced on the blocking path all along.
async fn send(call: HttpCall) -> LuaResult<(u16, String)> {
    tokio::task::spawn_blocking(move || call.run())
        .await
        .map_err(|e| LuaError::external(format!("std.http: spawn_blocking: {e}")))?
        .map_err(LuaError::external)
}

/// Replace `http.get` / `http.post` / `http.request` with versions that
/// do the network I/O on the blocking pool.
///
/// The policy check ([`crate::util::check_url`]), the config reads and the
/// agent selection all run on the VM thread while the [`HttpCall`] is
/// built, and [`build_response`] builds the `{status, body}` table there
/// too — the same functions the blocking module uses, so a rejected URL or
/// an unsupported method fails identically on both paths.
pub(super) fn install(lua: &Lua, http_tbl: &LuaTable) -> LuaResult<()> {
    http_tbl.set(
        "get",
        lua.create_async_function(|lua: Lua, url: String| async move {
            let call = prepare_get(&lua, url)?;
            let (status, body) = send(call).await?;
            build_response(&lua, status, body)
        })?,
    )?;

    http_tbl.set(
        "post",
        lua.create_async_function(
            |lua: Lua, (url, body, content_type): (String, String, Option<String>)| async move {
                let call = prepare_post(&lua, url, body, content_type)?;
                let (status, body) = send(call).await?;
                build_response(&lua, status, body)
            },
        )?,
    )?;

    http_tbl.set(
        "request",
        lua.create_async_function(|lua: Lua, opts: LuaTable| async move {
            let call = prepare_request(&lua, opts)?;
            let (status, body) = send(call).await?;
            build_response(&lua, status, body)
        })?,
    )?;

    Ok(())
}
