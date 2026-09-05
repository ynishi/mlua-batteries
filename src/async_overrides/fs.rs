//! Async `std.fs.*` — all 14 entries.

use mlua::prelude::*;

use crate::fs::{
    check_read_size, compile_glob, glob_base_dir, glob_entries, glob_table, strip_dot_slash,
    walk_entries, walk_table,
};
use crate::policy::PathOp;
use crate::util::{check_path, with_config};

/// [`crate::policy::FsAccess`] has to cross the `spawn_blocking` boundary:
/// `Direct` holds a `PathBuf` and `Capped` an `Arc<cap_std::fs::Dir>` (a
/// file-descriptor wrapper), so it is `Send`.  Asserted here so a future
/// variant that is not cannot land unnoticed.
const _: fn() = || {
    fn assert_send<T: Send + 'static>() {}
    assert_send::<crate::policy::FsAccess>();
};

/// Run `f` on the blocking pool and report a join failure as a Lua error.
///
/// `op` names the Lua function for the message; a join error means the
/// blocking task panicked or the runtime is shutting down, neither of
/// which the blocking path can produce, so it gets its own wording rather
/// than being disguised as an I/O error.
async fn blocking<T, F>(op: &'static str, f: F) -> LuaResult<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| LuaError::external(format!("{op}: spawn_blocking: {e}")))
}

/// Replace every `std.fs` entry with a version that does its I/O on the
/// blocking pool.
///
/// The split is the same in all 14: policy resolution ([`check_path`],
/// and [`check_read_size`] where the blocking version applies it) plus
/// argument and config reads happen on the VM thread, the I/O runs in
/// `spawn_blocking` and yields plain data (`String` / `Vec<u8>` / `bool` /
/// `Vec<String>`), and any Lua value is built back on the VM thread.
/// `walk` and `glob` go through [`walk_entries`] / [`glob_entries`], the
/// helpers the blocking module also calls, so the traversal and every
/// limit (`max_walk_depth`, `max_walk_entries`, `max_read_bytes`) are
/// enforced by the same code on both paths.
pub(super) fn install(lua: &Lua, fs_tbl: &LuaTable) -> LuaResult<()> {
    fs_tbl.set(
        "read",
        lua.create_async_function(|lua: Lua, path: String| async move {
            let access = check_path(&lua, &path, PathOp::Read)?;
            check_read_size(&lua, &access)?;
            blocking("std.fs.read", move || access.read_to_string())
                .await?
                .map_err(LuaError::external)
        })?,
    )?;

    fs_tbl.set(
        "write",
        lua.create_async_function(|lua: Lua, (path, content): (String, String)| async move {
            let access = check_path(&lua, &path, PathOp::Write)?;
            blocking("std.fs.write", move || access.write(content.as_bytes()))
                .await?
                .map_err(LuaError::external)?;
            Ok(true)
        })?,
    )?;

    fs_tbl.set(
        "read_binary",
        lua.create_async_function(|lua: Lua, path: String| async move {
            let access = check_path(&lua, &path, PathOp::Read)?;
            check_read_size(&lua, &access)?;
            let bytes = blocking("std.fs.read_binary", move || access.read_bytes())
                .await?
                .map_err(LuaError::external)?;
            lua.create_string(&bytes)
        })?,
    )?;

    fs_tbl.set(
        "write_binary",
        lua.create_async_function(
            |lua: Lua, (path, content): (String, mlua::LuaString)| async move {
                let access = check_path(&lua, &path, PathOp::Write)?;
                // Copy the bytes out of the VM before leaving the thread —
                // `mlua::String` borrows from the Lua heap and is `!Send`.
                let bytes = content.as_bytes().to_vec();
                blocking("std.fs.write_binary", move || access.write(&bytes))
                    .await?
                    .map_err(LuaError::external)?;
                Ok(true)
            },
        )?,
    )?;

    fs_tbl.set(
        "copy",
        lua.create_async_function(|lua: Lua, (src, dst): (String, String)| async move {
            let src_access = check_path(&lua, &src, PathOp::Read)?;
            let dst_access = check_path(&lua, &dst, PathOp::Write)?;
            blocking("std.fs.copy", move || src_access.copy_to(&dst_access))
                .await?
                .map_err(LuaError::external)?;
            Ok(true)
        })?,
    )?;

    fs_tbl.set(
        "rename",
        lua.create_async_function(|lua: Lua, (src, dst): (String, String)| async move {
            let src_access = check_path(&lua, &src, PathOp::Delete)?;
            let dst_access = check_path(&lua, &dst, PathOp::Write)?;
            blocking("std.fs.rename", move || src_access.rename_to(&dst_access))
                .await?
                .map_err(LuaError::external)?;
            Ok(true)
        })?,
    )?;

    #[cfg(unix)]
    fs_tbl.set(
        "symlink",
        lua.create_async_function(
            |lua: Lua, (target, linkpath): (String, String)| async move {
                let link_access = check_path(&lua, &linkpath, PathOp::Write)?;
                blocking("std.fs.symlink", move || {
                    link_access.symlink_to(std::path::Path::new(&target))
                })
                .await?
                .map_err(LuaError::external)?;
                Ok(true)
            },
        )?,
    )?;

    fs_tbl.set(
        "exists",
        lua.create_async_function(|lua: Lua, path: String| async move {
            let access = check_path(&lua, &path, PathOp::Read)?;
            blocking("std.fs.exists", move || access.exists()).await
        })?,
    )?;

    fs_tbl.set(
        "is_dir",
        lua.create_async_function(|lua: Lua, path: String| async move {
            let access = check_path(&lua, &path, PathOp::Read)?;
            blocking("std.fs.is_dir", move || access.is_dir()).await
        })?,
    )?;

    fs_tbl.set(
        "is_file",
        lua.create_async_function(|lua: Lua, path: String| async move {
            let access = check_path(&lua, &path, PathOp::Read)?;
            blocking("std.fs.is_file", move || access.is_file()).await
        })?,
    )?;

    fs_tbl.set(
        "mkdir",
        lua.create_async_function(|lua: Lua, path: String| async move {
            let access = check_path(&lua, &path, PathOp::Write)?;
            blocking("std.fs.mkdir", move || access.create_dir_all())
                .await?
                .map_err(LuaError::external)?;
            Ok(true)
        })?,
    )?;

    fs_tbl.set(
        "remove",
        lua.create_async_function(|lua: Lua, path: String| async move {
            let access = check_path(&lua, &path, PathOp::Delete)?;
            blocking("std.fs.remove", move || access.remove())
                .await?
                .map_err(LuaError::external)?;
            Ok(true)
        })?,
    )?;

    fs_tbl.set(
        "walk",
        lua.create_async_function(|lua: Lua, dir_path: String| async move {
            let access = check_path(&lua, &dir_path, PathOp::List)?;
            let (max_depth, max_entries) =
                with_config(&lua, |c| (c.max_walk_depth, c.max_walk_entries))?;

            let files = blocking("std.fs.walk", move || {
                walk_entries(&access, &dir_path, max_depth, max_entries)
            })
            .await?
            .map_err(LuaError::external)?;

            walk_table(&lua, files)
        })?,
    )?;

    fs_tbl.set(
        "glob",
        lua.create_async_function(|lua: Lua, pattern: String| async move {
            let (max_depth, max_entries) =
                with_config(&lua, |c| (c.max_walk_depth, c.max_walk_entries))?;

            // Normalize: strip leading "./" for consistent matching
            let normalized_pattern = strip_dot_slash(&pattern);

            // Compile glob pattern (pure, no FS access)
            let glob = compile_glob(normalized_pattern)?;

            // Extract base directory for walking
            let base_dir = glob_base_dir(normalized_pattern);

            // Resolve base directory through policy
            let access = check_path(&lua, &base_dir, PathOp::List)?;

            let files = blocking("std.fs.glob", move || {
                glob_entries(&access, &base_dir, &glob, max_depth, max_entries)
            })
            .await?
            .map_err(LuaError::external)?;

            glob_table(&lua, files)
        })?,
    )?;

    Ok(())
}
