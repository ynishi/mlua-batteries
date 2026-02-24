//! Path manipulation module.
//!
//! Most functions are pure computation with no I/O.
//! Exception: `absolute` performs filesystem access (symlink resolution
//! via `canonicalize`) and is subject to the active
//! [`PathPolicy`](crate::policy::PathPolicy).
//!
//! # `absolute` and sandboxed mode
//!
//! `path.absolute(p)` internally calls [`std::fs::canonicalize`], which
//! resolves symlinks and returns an **absolute** path.  This is **not
//! available** in [`Sandboxed`](crate::policy::Sandboxed) mode.
//!
//! ## Why it doesn't work
//!
//! [`cap_std::fs::Dir`] *does* have a `canonicalize` method, but it
//! returns a **relative** path (relative to the `Dir` handle) because
//! absolute paths break the capability model.  `path.absolute` promises
//! callers an absolute path, and [`FsAccess`](crate::policy::FsAccess)
//! does not hold the sandbox root path, so there is no way to convert
//! `cap_std`'s relative result back to an absolute path.
//!
//! Ref: <https://docs.rs/cap-std/4.0.2/cap_std/fs/struct.Dir.html#method.canonicalize>
//!
//! ## Workaround for sandboxed environments
//!
//! Use the pure-computation functions that require no filesystem access:
//!
//! ```lua
//! -- Check if already absolute
//! if not path.is_absolute(p) then
//!     -- Build absolute path from a known base
//!     p = path.join(base_dir, p)
//! end
//! ```
//!
//! ## Future: `std::path::absolute` (Rust 1.79+)
//!
//! [`std::path::absolute`] (stabilized in Rust 1.79.0) makes a path
//! absolute **without** filesystem access — it does not resolve symlinks
//! and works even if the path does not exist.  Migrating to this
//! function would allow `path.absolute` to work in sandboxed mode,
//! but changes the semantics (symlinks would no longer be resolved).
//!
//! Ref: <https://doc.rust-lang.org/std/path/fn.absolute.html>
//! Ref: <https://github.com/rust-lang/rust/pull/124335>
//!
//! # Encoding — UTF-8 only (by design)
//!
//! All path arguments are received as Rust [`String`] (UTF-8).
//! Non-UTF-8 Lua strings are rejected at the `FromLua` boundary
//! with `FromLuaConversionError`.  Returned paths use
//! [`to_string_lossy`](std::path::Path::to_string_lossy),
//! replacing any non-UTF-8 bytes with U+FFFD.
//!
//! Raw byte (`OsStr`) round-tripping is intentionally unsupported.
//! mlua's `FromLua for String` enforces UTF-8 validation before
//! handler code runs, so supporting raw bytes would require every
//! function to accept `mlua::String` + `as_bytes()` and return
//! via `OsStr::as_bytes()`.  The added complexity is not justified
//! given the rarity of non-UTF-8 filenames on modern systems.
//!
//! Ref: <https://docs.rs/mlua/latest/mlua/struct.String.html>
//!
//! ```lua
//! local path = std.path
//! local p = path.join("/usr", "local", "bin")
//! local dir = path.parent("/usr/local/bin/foo")
//! local name = path.filename("/usr/local/config.toml")
//! local base = path.stem("/usr/local/config.toml")
//! local ext = path.ext("/usr/local/config.toml")
//! ```

use mlua::prelude::*;
use std::path::{Path, PathBuf};

pub fn module(lua: &Lua) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;

    t.set(
        "join",
        lua.create_function(|_, parts: LuaMultiValue| {
            let mut path = PathBuf::new();
            for (i, part) in parts.iter().enumerate() {
                match part {
                    LuaValue::String(s) => path.push(&*s.to_str()?),
                    other => {
                        return Err(LuaError::external(format!(
                            "path.join: argument {} must be a string, got {}",
                            i + 1,
                            other.type_name()
                        )));
                    }
                }
            }
            Ok(path.to_string_lossy().to_string())
        })?,
    )?;

    t.set(
        "parent",
        lua.create_function(|_, p: String| {
            Ok(Path::new(&p)
                .parent()
                .map(|p| p.to_string_lossy().to_string()))
        })?,
    )?;

    t.set(
        "filename",
        lua.create_function(|_, p: String| {
            Ok(Path::new(&p)
                .file_name()
                .map(|f| f.to_string_lossy().to_string()))
        })?,
    )?;

    t.set(
        "stem",
        lua.create_function(|_, p: String| {
            Ok(Path::new(&p)
                .file_stem()
                .map(|f| f.to_string_lossy().to_string()))
        })?,
    )?;

    t.set(
        "ext",
        lua.create_function(|_, p: String| {
            Ok(Path::new(&p)
                .extension()
                .map(|e| e.to_string_lossy().to_string()))
        })?,
    )?;

    // path.absolute(p) — resolve a path to its absolute, canonical form.
    //
    // Uses `std::fs::canonicalize` (which resolves symlinks) rather than
    // `std::path::absolute` (Rust 1.79+, purely lexical).  The symlink
    // resolution is intentional: callers of `path.absolute` in scripting
    // contexts typically expect the "real" path on disk (e.g. to compare
    // two paths that may traverse symlinks).
    //
    // Trade-off: requires the path to exist and performs I/O.
    // `std::path::absolute` would avoid both but changes semantics.
    // See the module-level doc for a detailed discussion and the
    // sandboxed-mode workaround.
    t.set(
        "absolute",
        lua.create_function(|lua, p: String| {
            let access = crate::util::check_path(lua, &p, crate::policy::PathOp::Read)?;
            access
                .canonicalize()
                .map(|p| p.to_string_lossy().to_string())
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::Unsupported {
                        LuaError::external(
                            "path.absolute is not supported in sandboxed mode: \
                             cap_std canonicalize returns a relative path but \
                             path.absolute must return an absolute path. \
                             Use path.is_absolute() + path.join() instead.",
                        )
                    } else {
                        LuaError::external(e)
                    }
                })
        })?,
    )?;

    t.set(
        "is_absolute",
        lua.create_function(|_, p: String| Ok(Path::new(&p).is_absolute()))?,
    )?;

    Ok(t)
}

#[cfg(test)]
mod tests {
    use mlua::Lua;

    use crate::util::test_eval as eval;

    #[test]
    fn join_parts() {
        let s: String = eval(r#"return std.path.join("/usr", "local", "bin")"#);
        assert_eq!(s, "/usr/local/bin");
    }

    #[test]
    fn parent_of_file() {
        let s: String = eval(r#"return std.path.parent("/usr/local/bin/foo")"#);
        assert_eq!(s, "/usr/local/bin");
    }

    #[test]
    fn filename_extraction() {
        let s: String = eval(r#"return std.path.filename("/usr/local/config.toml")"#);
        assert_eq!(s, "config.toml");
    }

    #[test]
    fn stem_without_extension() {
        let s: String = eval(r#"return std.path.stem("/usr/local/config.toml")"#);
        assert_eq!(s, "config");
    }

    #[test]
    fn ext_extraction() {
        let s: String = eval(r#"return std.path.ext("/usr/local/config.toml")"#);
        assert_eq!(s, "toml");
    }

    #[test]
    fn is_absolute_true() {
        let b: bool = eval(r#"return std.path.is_absolute("/usr/local")"#);
        assert!(b);
    }

    #[test]
    fn is_absolute_false() {
        let b: bool = eval(r#"return std.path.is_absolute("relative/path")"#);
        assert!(!b);
    }

    #[test]
    fn parent_of_root_is_nil() {
        let b: bool = eval(
            r#"
            return std.path.parent("/") == nil
        "#,
        );
        assert!(b);
    }

    #[test]
    fn absolute_resolves_existing_path() {
        let s: String = eval(r#"return std.path.absolute("/tmp")"#);
        assert!(std::path::Path::new(&s).is_absolute());
    }

    #[test]
    fn absolute_nonexistent_returns_error() {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let result: mlua::Result<mlua::Value> = lua
            .load(r#"return std.path.absolute("/nonexistent_mlua_bat_xyz")"#)
            .eval();
        assert!(result.is_err());
    }

    #[cfg(feature = "sandbox")]
    #[test]
    fn absolute_sandboxed_returns_clear_error() {
        let sandbox = std::env::temp_dir().join("mlua_bat_test_path_sandbox");
        std::fs::create_dir_all(&sandbox).unwrap();
        std::fs::write(sandbox.join("file.txt"), "").unwrap();

        let lua = Lua::new();
        let config = crate::config::Config::builder()
            .path_policy(crate::policy::Sandboxed::new([&sandbox]).unwrap())
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        let path_str = sandbox.join("file.txt").to_string_lossy().to_string();
        let code = format!(r#"return std.path.absolute("{path_str}")"#);
        let result: mlua::Result<mlua::Value> = lua.load(&code).eval();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("path.absolute is not supported in sandboxed mode"),
            "error message should mention path.absolute and sandboxed mode, got: {err_msg}"
        );

        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn join_rejects_non_string_argument() {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let result: mlua::Result<mlua::Value> = lua
            .load(r#"return std.path.join("/usr", 42, "bin")"#)
            .eval();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("must be a string"));
    }

    #[test]
    fn join_with_empty_string() {
        let s: String = eval(r#"return std.path.join("/usr", "", "bin")"#);
        assert_eq!(s, "/usr/bin");
    }

    #[test]
    fn join_single_argument() {
        let s: String = eval(r#"return std.path.join("foo")"#);
        assert_eq!(s, "foo");
    }

    #[test]
    fn parent_of_single_component() {
        // "foo" → parent is ""
        let b: bool = eval(
            r#"
            local p = std.path.parent("foo")
            return p == ""
        "#,
        );
        assert!(b);
    }

    #[test]
    fn ext_no_extension_returns_nil() {
        let b: bool = eval(r#"return std.path.ext("Makefile") == nil"#);
        assert!(b);
    }

    #[test]
    fn filename_of_root_is_nil() {
        let b: bool = eval(r#"return std.path.filename("/") == nil"#);
        assert!(b);
    }
}
