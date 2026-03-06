//! Filesystem operations module.
//!
//! All path-accepting functions are subject to the active
//! [`PathPolicy`](crate::policy::PathPolicy).
//!
//! In [`Sandboxed`](crate::policy::Sandboxed) mode, I/O goes through
//! [`cap_std::fs::Dir`] handles, preventing path traversal at the OS level.
//!
//! ```lua
//! local fs = std.fs
//! local content = fs.read("file.txt")
//! fs.write("out.txt", content)
//! local bytes = fs.read_binary("image.png")  -- raw bytes as Lua string
//! fs.write_binary("copy.png", bytes)
//! fs.copy("src.txt", "dst.txt")
//! fs.mkdir("a/b/c")
//! fs.remove("old.txt")
//! if fs.is_file("test.txt") then ... end
//! if fs.is_dir("src") then ... end
//! local files = fs.walk("./src")
//! local matches = fs.glob("*.rs")
//! ```

use std::path::{Path, PathBuf};

use mlua::prelude::*;

use crate::policy::PathOp;
use crate::util::{check_path, with_config};

/// Extract the base directory for walking from a glob pattern.
///
/// Returns the longest path prefix that contains no wildcard characters.
/// For patterns without wildcards (literal paths), returns the parent directory.
fn glob_base_dir(pattern: &str) -> String {
    let path = Path::new(pattern);
    let mut base = PathBuf::new();
    let mut found_wildcard = false;

    for component in path.components() {
        let s = component.as_os_str().to_string_lossy();
        if s.contains('*') || s.contains('?') || s.contains('[') || s.contains('{') {
            found_wildcard = true;
            break;
        }
        base.push(component);
    }

    if !found_wildcard {
        // Literal path — walk from parent directory
        return path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());
    }

    if base.as_os_str().is_empty() {
        ".".to_string()
    } else {
        base.to_string_lossy().to_string()
    }
}

/// Strip leading `"./"` prefix for consistent path matching and output.
fn strip_dot_slash(path: &str) -> &str {
    path.strip_prefix("./").unwrap_or(path)
}

/// Return an error if the file exceeds `Config::max_read_bytes` (when set).
fn check_read_size(lua: &Lua, access: &crate::policy::FsAccess) -> LuaResult<()> {
    let limit = with_config(lua, |c| c.max_read_bytes)?;
    if let Some(max) = limit {
        let size = access.file_size().map_err(LuaError::external)?;
        if size > max {
            return Err(LuaError::external(format!(
                "file size {size} bytes exceeds max_read_bytes limit ({max} bytes)"
            )));
        }
    }
    Ok(())
}

pub fn module(lua: &Lua) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;

    t.set(
        "read",
        lua.create_function(|lua, path: String| {
            let access = check_path(lua, &path, PathOp::Read)?;
            check_read_size(lua, &access)?;
            access.read_to_string().map_err(LuaError::external)
        })?,
    )?;

    t.set(
        "write",
        lua.create_function(|lua, (path, content): (String, String)| {
            let access = check_path(lua, &path, PathOp::Write)?;
            access
                .write(content.as_bytes())
                .map_err(LuaError::external)?;
            Ok(true)
        })?,
    )?;

    t.set(
        "read_binary",
        lua.create_function(|lua, path: String| {
            let access = check_path(lua, &path, PathOp::Read)?;
            check_read_size(lua, &access)?;
            let bytes = access.read_bytes().map_err(LuaError::external)?;
            lua.create_string(&bytes)
        })?,
    )?;

    t.set(
        "write_binary",
        lua.create_function(|lua, (path, content): (String, mlua::String)| {
            let access = check_path(lua, &path, PathOp::Write)?;
            access
                .write(content.as_bytes())
                .map_err(LuaError::external)?;
            Ok(true)
        })?,
    )?;

    t.set(
        "copy",
        lua.create_function(|lua, (src, dst): (String, String)| {
            let src_access = check_path(lua, &src, PathOp::Read)?;
            let dst_access = check_path(lua, &dst, PathOp::Write)?;
            src_access
                .copy_to(&dst_access)
                .map_err(LuaError::external)?;
            Ok(true)
        })?,
    )?;

    t.set(
        "exists",
        lua.create_function(|lua, path: String| {
            let access = check_path(lua, &path, PathOp::Read)?;
            Ok(access.exists())
        })?,
    )?;

    t.set(
        "is_dir",
        lua.create_function(|lua, path: String| {
            let access = check_path(lua, &path, PathOp::Read)?;
            Ok(access.is_dir())
        })?,
    )?;

    t.set(
        "is_file",
        lua.create_function(|lua, path: String| {
            let access = check_path(lua, &path, PathOp::Read)?;
            Ok(access.is_file())
        })?,
    )?;

    t.set(
        "mkdir",
        lua.create_function(|lua, path: String| {
            let access = check_path(lua, &path, PathOp::Write)?;
            access.create_dir_all().map_err(LuaError::external)?;
            Ok(true)
        })?,
    )?;

    t.set(
        "remove",
        lua.create_function(|lua, path: String| {
            let access = check_path(lua, &path, PathOp::Delete)?;
            access.remove().map_err(LuaError::external)?;
            Ok(true)
        })?,
    )?;

    t.set(
        "walk",
        lua.create_function(|lua, dir_path: String| {
            let access = check_path(lua, &dir_path, PathOp::List)?;
            let (max_depth, max_entries) =
                with_config(lua, |c| (c.max_walk_depth, c.max_walk_entries))?;

            let files = access
                .walk_files(Path::new(&dir_path), max_depth, max_entries)
                .map_err(|e| LuaError::external(format!("fs.walk: {e}")))?;

            let table = lua.create_table()?;
            for (i, path) in files.into_iter().enumerate() {
                table.set(i + 1, path)?;
            }
            Ok(table)
        })?,
    )?;

    t.set(
        "glob",
        lua.create_function(|lua, pattern: String| {
            let (max_depth, max_entries) =
                with_config(lua, |c| (c.max_walk_depth, c.max_walk_entries))?;

            // Normalize: strip leading "./" for consistent matching
            let normalized_pattern = strip_dot_slash(&pattern);

            // Compile glob pattern (pure, no FS access)
            let glob = globset::GlobBuilder::new(normalized_pattern)
                .literal_separator(true)
                .build()
                .map_err(|e| LuaError::external(format!("fs.glob: invalid pattern: {e}")))?
                .compile_matcher();

            // Extract base directory for walking
            let base_dir = glob_base_dir(normalized_pattern);

            // Resolve base directory through policy
            let access = check_path(lua, &base_dir, PathOp::List)?;

            let files = access
                .walk_files_filtered(
                    Path::new(&base_dir),
                    &|path_str| {
                        let m = strip_dot_slash(path_str);
                        glob.is_match(m)
                    },
                    max_depth,
                    max_entries,
                )
                .map_err(|e| LuaError::external(format!("fs.glob: {e}")))?;

            let table = lua.create_table()?;
            for (i, path) in files.into_iter().enumerate() {
                let normalized = strip_dot_slash(&path);
                table.set(i + 1, normalized.to_string())?;
            }

            Ok(table)
        })?,
    )?;

    Ok(t)
}

#[cfg(test)]
mod tests {
    use mlua::Lua;

    use crate::util::test_eval as eval;

    #[test]
    fn read_and_write_file() {
        let dir = std::env::temp_dir().join("mlua_bat_test_fs_rw");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.txt");
        let path_str = path.to_string_lossy();

        let b: bool = eval(&format!(
            r#"
            std.fs.write("{path_str}", "hello mlua-batteries")
            return std.fs.read("{path_str}") == "hello mlua-batteries"
        "#
        ));
        assert!(b);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exists_and_is_dir() {
        let dir = std::env::temp_dir().join("mlua_bat_test_fs_exists");
        std::fs::create_dir_all(&dir).unwrap();
        let dir_str = dir.to_string_lossy();

        let b: bool = eval(&format!(
            r#"
            return std.fs.exists("{dir_str}") and std.fs.is_dir("{dir_str}")
        "#
        ));
        assert!(b);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn is_file_true_for_file() {
        let dir = std::env::temp_dir().join("mlua_bat_test_fs_is_file");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("test.txt");
        std::fs::write(&file, "content").unwrap();
        let file_str = file.to_string_lossy();

        let b: bool = eval(&format!(r#"return std.fs.is_file("{file_str}")"#));
        assert!(b);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn is_file_false_for_dir() {
        let dir = std::env::temp_dir().join("mlua_bat_test_fs_is_file_dir");
        std::fs::create_dir_all(&dir).unwrap();
        let dir_str = dir.to_string_lossy();

        let b: bool = eval(&format!(r#"return std.fs.is_file("{dir_str}")"#));
        assert!(!b);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn is_file_false_for_nonexistent() {
        let b: bool = eval(r#"return std.fs.is_file("/tmp/__mlua_bat_nonexistent_is_file__")"#);
        assert!(!b);
    }

    #[test]
    fn mkdir_creates_nested() {
        let base = std::env::temp_dir().join("mlua_bat_test_fs_mkdir");
        let _ = std::fs::remove_dir_all(&base);
        let nested = base.join("a").join("b").join("c");
        let nested_str = nested.to_string_lossy();

        let b: bool = eval(&format!(
            r#"
            std.fs.mkdir("{nested_str}")
            return std.fs.is_dir("{nested_str}")
        "#
        ));
        assert!(b);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn copy_file() {
        let dir = std::env::temp_dir().join("mlua_bat_test_fs_copy");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.txt");
        let dst = dir.join("dst.txt");
        std::fs::write(&src, "copy me").unwrap();
        let src_str = src.to_string_lossy();
        let dst_str = dst.to_string_lossy();

        let s: String = eval(&format!(
            r#"
            std.fs.copy("{src_str}", "{dst_str}")
            return std.fs.read("{dst_str}")
        "#
        ));
        assert_eq!(s, "copy me");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_file() {
        let dir = std::env::temp_dir().join("mlua_bat_test_fs_remove");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("to_remove.txt");
        std::fs::write(&file, "delete me").unwrap();
        let file_str = file.to_string_lossy();

        let b: bool = eval(&format!(
            r#"
            std.fs.remove("{file_str}")
            return not std.fs.exists("{file_str}")
        "#
        ));
        assert!(b);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn walk_returns_files() {
        let dir = std::env::temp_dir().join("mlua_bat_test_fs_walk");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "").unwrap();
        std::fs::write(dir.join("sub").join("b.txt"), "").unwrap();
        let dir_str = dir.to_string_lossy();

        let n: i64 = eval(&format!(
            r#"
            return #std.fs.walk("{dir_str}")
        "#
        ));
        assert_eq!(n, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn glob_matches_pattern() {
        let dir = std::env::temp_dir().join("mlua_bat_test_fs_glob");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "").unwrap();
        std::fs::write(dir.join("b.txt"), "").unwrap();
        std::fs::write(dir.join("c.rs"), "").unwrap();
        let pattern = dir.join("*.txt").to_string_lossy().to_string();

        let n: i64 = eval(&format!(
            r#"
            return #std.fs.glob("{pattern}")
        "#
        ));
        assert_eq!(n, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_directory() {
        let dir = std::env::temp_dir().join("mlua_bat_test_fs_remove_dir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("file.txt"), "x").unwrap();
        let dir_str = dir.to_string_lossy();

        let b: bool = eval(&format!(
            r#"
            std.fs.remove("{dir_str}")
            return not std.fs.exists("{dir_str}")
        "#
        ));
        assert!(b);
    }

    #[test]
    fn read_nonexistent_returns_error() {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let result: mlua::Result<mlua::Value> = lua
            .load(r#"return std.fs.read("/tmp/__mlua_bat_nonexistent__")"#)
            .eval();
        assert!(result.is_err());
    }

    // ─── Policy integration tests (sandbox feature) ─────────

    #[cfg(feature = "sandbox")]
    #[test]
    fn sandboxed_blocks_outside_read() {
        let lua = Lua::new();
        let sandbox_dir = std::env::temp_dir().join("mlua_bat_test_sandbox_fs");
        std::fs::create_dir_all(&sandbox_dir).unwrap();

        let config = crate::config::Config::builder()
            .path_policy(crate::policy::Sandboxed::new([&sandbox_dir]).unwrap())
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        // Reading /etc/hosts (outside sandbox) should fail
        let result: mlua::Result<mlua::Value> =
            lua.load(r#"return std.fs.read("/etc/hosts")"#).eval();
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&sandbox_dir);
    }

    #[cfg(feature = "sandbox")]
    #[test]
    fn sandboxed_allows_inside_write() {
        let lua = Lua::new();
        let sandbox_dir = std::env::temp_dir().join("mlua_bat_test_sandbox_fs_write");
        std::fs::create_dir_all(&sandbox_dir).unwrap();
        let file_str = sandbox_dir.join("test.txt").to_string_lossy().to_string();

        let config = crate::config::Config::builder()
            .path_policy(crate::policy::Sandboxed::new([&sandbox_dir]).unwrap())
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        let result: mlua::Result<bool> = lua
            .load(&format!(r#"return std.fs.write("{file_str}", "ok")"#))
            .eval();
        assert!(result.is_ok());

        let _ = std::fs::remove_dir_all(&sandbox_dir);
    }

    #[cfg(feature = "sandbox")]
    #[test]
    fn read_only_blocks_write() {
        let lua = Lua::new();
        let sandbox_dir = std::env::temp_dir().join("mlua_bat_test_readonly_fs");
        std::fs::create_dir_all(&sandbox_dir).unwrap();
        let file_str = sandbox_dir.join("test.txt").to_string_lossy().to_string();

        let config = crate::config::Config::builder()
            .path_policy(
                crate::policy::Sandboxed::new([&sandbox_dir])
                    .unwrap()
                    .read_only(),
            )
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        let result: mlua::Result<mlua::Value> = lua
            .load(&format!(r#"return std.fs.write("{file_str}", "x")"#))
            .eval();
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&sandbox_dir);
    }

    #[cfg(feature = "sandbox")]
    #[test]
    fn sandboxed_walk_stays_within_sandbox() {
        let lua = Lua::new();
        let sandbox_dir = std::env::temp_dir().join("mlua_bat_test_sandbox_walk");
        let _ = std::fs::remove_dir_all(&sandbox_dir);
        std::fs::create_dir_all(sandbox_dir.join("sub")).unwrap();
        std::fs::write(sandbox_dir.join("a.txt"), "").unwrap();
        std::fs::write(sandbox_dir.join("sub").join("b.txt"), "").unwrap();
        let dir_str = sandbox_dir.to_string_lossy();

        let config = crate::config::Config::builder()
            .path_policy(crate::policy::Sandboxed::new([&sandbox_dir]).unwrap())
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        let n: i64 = lua
            .load(&format!(r#"return #std.fs.walk("{dir_str}")"#))
            .eval()
            .unwrap();
        assert_eq!(n, 2);

        let _ = std::fs::remove_dir_all(&sandbox_dir);
    }

    #[cfg(feature = "sandbox")]
    #[test]
    fn sandboxed_glob_stays_within_sandbox() {
        let lua = Lua::new();
        let sandbox_dir = std::env::temp_dir().join("mlua_bat_test_sandbox_glob");
        let _ = std::fs::remove_dir_all(&sandbox_dir);
        std::fs::create_dir_all(&sandbox_dir).unwrap();
        std::fs::write(sandbox_dir.join("a.txt"), "").unwrap();
        std::fs::write(sandbox_dir.join("b.txt"), "").unwrap();
        std::fs::write(sandbox_dir.join("c.rs"), "").unwrap();
        let pattern = sandbox_dir.join("*.txt").to_string_lossy().to_string();

        let config = crate::config::Config::builder()
            .path_policy(crate::policy::Sandboxed::new([&sandbox_dir]).unwrap())
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        let n: i64 = lua
            .load(&format!(r#"return #std.fs.glob("{pattern}")"#))
            .eval()
            .unwrap();
        assert_eq!(n, 2);

        let _ = std::fs::remove_dir_all(&sandbox_dir);
    }

    #[cfg(feature = "sandbox")]
    #[test]
    fn sandboxed_blocks_read_binary_outside() {
        let lua = Lua::new();
        let sandbox_dir = std::env::temp_dir().join("mlua_bat_test_sandbox_read_binary");
        std::fs::create_dir_all(&sandbox_dir).unwrap();

        let config = crate::config::Config::builder()
            .path_policy(crate::policy::Sandboxed::new([&sandbox_dir]).unwrap())
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        let result: mlua::Result<mlua::Value> = lua
            .load(r#"return std.fs.read_binary("/etc/hosts")"#)
            .eval();
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&sandbox_dir);
    }

    #[cfg(feature = "sandbox")]
    #[test]
    fn read_only_blocks_write_binary() {
        let lua = Lua::new();
        let sandbox_dir = std::env::temp_dir().join("mlua_bat_test_readonly_write_binary");
        std::fs::create_dir_all(&sandbox_dir).unwrap();
        let file_str = sandbox_dir.join("test.bin").to_string_lossy().to_string();

        let config = crate::config::Config::builder()
            .path_policy(
                crate::policy::Sandboxed::new([&sandbox_dir])
                    .unwrap()
                    .read_only(),
            )
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        let result: mlua::Result<mlua::Value> = lua
            .load(&format!(
                r#"return std.fs.write_binary("{file_str}", "\x00")"#
            ))
            .eval();
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&sandbox_dir);
    }

    #[cfg(feature = "sandbox")]
    #[test]
    fn sandboxed_glob_blocks_outside_pattern() {
        let lua = Lua::new();
        let sandbox_dir = std::env::temp_dir().join("mlua_bat_test_sandbox_glob_block");
        std::fs::create_dir_all(&sandbox_dir).unwrap();

        let config = crate::config::Config::builder()
            .path_policy(crate::policy::Sandboxed::new([&sandbox_dir]).unwrap())
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        // Glob pattern pointing outside sandbox should fail at base_dir resolution
        let result: mlua::Result<mlua::Value> = lua.load(r#"return std.fs.glob("/etc/*")"#).eval();
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&sandbox_dir);
    }

    #[test]
    fn glob_recursive_pattern() {
        let dir = std::env::temp_dir().join("mlua_bat_test_fs_glob_recursive");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "").unwrap();
        std::fs::write(dir.join("sub").join("b.txt"), "").unwrap();
        std::fs::write(dir.join("sub").join("c.rs"), "").unwrap();
        let pattern = dir.join("**/*.txt").to_string_lossy().to_string();

        let n: i64 = eval(&format!(
            r#"
            return #std.fs.glob("{pattern}")
        "#
        ));
        assert_eq!(n, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn glob_no_match_returns_empty() {
        let dir = std::env::temp_dir().join("mlua_bat_test_fs_glob_empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("file.txt"), "").unwrap();
        let pattern = dir.join("*.xyz").to_string_lossy().to_string();

        let n: i64 = eval(&format!(
            r#"
            return #std.fs.glob("{pattern}")
        "#
        ));
        assert_eq!(n, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ─── glob_base_dir unit tests ─────────────────

    #[test]
    fn glob_base_dir_wildcard_at_start() {
        assert_eq!(super::glob_base_dir("*.rs"), ".");
    }

    #[test]
    fn glob_base_dir_with_prefix() {
        assert_eq!(super::glob_base_dir("src/*.rs"), "src");
    }

    #[test]
    fn glob_base_dir_recursive_wildcard() {
        assert_eq!(super::glob_base_dir("src/**/*.rs"), "src");
    }

    #[test]
    fn glob_base_dir_absolute() {
        assert_eq!(super::glob_base_dir("/app/data/*.txt"), "/app/data");
    }

    #[test]
    fn glob_base_dir_literal_file() {
        // No wildcards → return parent directory
        assert_eq!(super::glob_base_dir("src/main.rs"), "src");
    }

    #[test]
    fn glob_base_dir_literal_file_no_dir() {
        // No wildcards, no directory → return "."
        assert_eq!(super::glob_base_dir("main.rs"), ".");
    }

    #[test]
    fn glob_base_dir_double_star_at_start() {
        assert_eq!(super::glob_base_dir("**/*.rs"), ".");
    }

    // ─── additional edge-case tests ───────────────

    #[test]
    fn write_nonexistent_parent_returns_error() {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let result: mlua::Result<mlua::Value> = lua
            .load(r#"return std.fs.write("/tmp/__mlua_bat_no_parent__/deep/file.txt", "x")"#)
            .eval();
        assert!(result.is_err());
    }

    #[test]
    fn read_binary_file() {
        let dir = std::env::temp_dir().join("mlua_bat_test_fs_binary");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bin.dat");
        // Write valid UTF-8 bytes — read should succeed
        std::fs::write(&path, b"hello\xc3\xa9").unwrap();
        let path_str = path.to_string_lossy();

        let s: String = eval(&format!(r#"return std.fs.read("{path_str}")"#));
        assert_eq!(s, "hello\u{e9}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_binary_preserves_raw_bytes() {
        let dir = std::env::temp_dir().join("mlua_bat_test_fs_read_binary");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("raw.bin");
        // Write bytes including non-UTF-8 and null
        let raw: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47, 0x00, 0xFF, 0xFE];
        std::fs::write(&path, &raw).unwrap();
        let path_str = path.to_string_lossy();

        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let result: mlua::String = lua
            .load(&format!(r#"return std.fs.read_binary("{path_str}")"#))
            .eval()
            .unwrap();
        assert_eq!(result.as_bytes(), &raw);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_binary_nonexistent_returns_error() {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let result: mlua::Result<mlua::Value> = lua
            .load(r#"return std.fs.read_binary("/tmp/__mlua_bat_read_binary_nonexistent__")"#)
            .eval();
        assert!(result.is_err());
    }

    #[test]
    fn write_binary_and_read_back() {
        let dir = std::env::temp_dir().join("mlua_bat_test_fs_write_binary");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.bin");
        let path_str = path.to_string_lossy();

        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();

        // Write raw bytes (including null and non-UTF-8) via Lua
        let result: bool = lua
            .load(&format!(
                r#"return std.fs.write_binary("{path_str}", "\x89PNG\x00\xFF\xFE")"#
            ))
            .eval()
            .unwrap();
        assert!(result);

        let written = std::fs::read(&path).unwrap();
        assert_eq!(written, vec![0x89, 0x50, 0x4E, 0x47, 0x00, 0xFF, 0xFE]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn binary_roundtrip() {
        let dir = std::env::temp_dir().join("mlua_bat_test_fs_binary_roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("roundtrip.bin");
        let path_str = path.to_string_lossy();

        // Write raw bytes from Rust, read via read_binary, write via write_binary
        let original: Vec<u8> = (0..=255).collect();
        std::fs::write(&path, &original).unwrap();

        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();

        let n: i64 = lua
            .load(&format!(
                r#"
                local data = std.fs.read_binary("{path_str}")
                return #data
            "#
            ))
            .eval()
            .unwrap();
        assert_eq!(n, 256);

        // Roundtrip: read → write → verify
        let dst = dir.join("copy.bin");
        let dst_str = dst.to_string_lossy();
        lua.load(&format!(
            r#"
            local data = std.fs.read_binary("{path_str}")
            std.fs.write_binary("{dst_str}", data)
        "#
        ))
        .exec()
        .unwrap();

        let copied = std::fs::read(&dst).unwrap();
        assert_eq!(copied, original);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_respects_max_read_bytes() {
        let lua = Lua::new();
        let dir = std::env::temp_dir().join("mlua_bat_test_max_read_bytes");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.txt");
        std::fs::write(&path, "hello world").unwrap(); // 11 bytes
        let path_str = path.to_string_lossy();

        let config = crate::config::Config::builder()
            .max_read_bytes(5)
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        let result: mlua::Result<mlua::Value> = lua
            .load(&format!(r#"return std.fs.read("{path_str}")"#))
            .eval();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("max_read_bytes"),
            "expected max_read_bytes error, got: {err_msg}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_binary_respects_max_read_bytes() {
        let lua = Lua::new();
        let dir = std::env::temp_dir().join("mlua_bat_test_max_read_bytes_binary");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.bin");
        std::fs::write(&path, vec![0u8; 100]).unwrap();
        let path_str = path.to_string_lossy();

        let config = crate::config::Config::builder()
            .max_read_bytes(50)
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        let result: mlua::Result<mlua::Value> = lua
            .load(&format!(r#"return std.fs.read_binary("{path_str}")"#))
            .eval();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("max_read_bytes"),
            "expected max_read_bytes error, got: {err_msg}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_within_max_read_bytes_succeeds() {
        let lua = Lua::new();
        let dir = std::env::temp_dir().join("mlua_bat_test_max_read_ok");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("small.txt");
        std::fs::write(&path, "hi").unwrap(); // 2 bytes
        let path_str = path.to_string_lossy();

        let config = crate::config::Config::builder()
            .max_read_bytes(1024)
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        let s: String = lua
            .load(&format!(r#"return std.fs.read("{path_str}")"#))
            .eval()
            .unwrap();
        assert_eq!(s, "hi");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_exact_boundary_succeeds() {
        let lua = Lua::new();
        let dir = std::env::temp_dir().join("mlua_bat_test_max_read_boundary");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("exact.txt");
        std::fs::write(&path, "12345").unwrap(); // exactly 5 bytes
        let path_str = path.to_string_lossy();

        let config = crate::config::Config::builder()
            .max_read_bytes(5)
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        let s: String = lua
            .load(&format!(r#"return std.fs.read("{path_str}")"#))
            .eval()
            .unwrap();
        assert_eq!(s, "12345");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn walk_respects_max_depth_zero() {
        // max_walk_depth=0 means only the root itself (no files)
        let lua = Lua::new();
        let dir = std::env::temp_dir().join("mlua_bat_test_walk_depth0");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "").unwrap();
        std::fs::write(dir.join("sub").join("b.txt"), "").unwrap();
        let dir_str = dir.to_string_lossy();

        let config = crate::config::Config::builder()
            .max_walk_depth(0)
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        let n: i64 = lua
            .load(&format!(r#"return #std.fs.walk("{dir_str}")"#))
            .eval()
            .unwrap();
        // depth 0 = root dir only, no descent into files
        assert_eq!(n, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn walk_entry_limit_returns_error() {
        let lua = Lua::new();
        let dir = std::env::temp_dir().join("mlua_bat_test_walk_limit");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "").unwrap();
        std::fs::write(dir.join("b.txt"), "").unwrap();
        std::fs::write(dir.join("c.txt"), "").unwrap();
        let dir_str = dir.to_string_lossy();

        let config = crate::config::Config::builder()
            .max_walk_entries(1)
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        let result: mlua::Result<mlua::Value> = lua
            .load(&format!(r#"return std.fs.walk("{dir_str}")"#))
            .eval();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("entry limit exceeded"),
            "expected entry limit error, got: {err_msg}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_preserves_error_on_permission_denied() {
        // Verify that remove() returns an io::Error (not panic) for
        // paths that genuinely don't exist
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let result: mlua::Result<mlua::Value> = lua
            .load(r#"return std.fs.remove("/tmp/__mlua_bat_remove_nonexistent__")"#)
            .eval();
        assert!(result.is_err());
    }
}
