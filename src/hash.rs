//! Hashing module (SHA-256).
//!
//! ```lua
//! local hash = std.hash
//! local h = hash.sha256("hello")
//! local h2 = hash.sha256_file("/path/to/file")
//! ```

use mlua::prelude::*;
use sha2::{Digest, Sha256};

use crate::policy::PathOp;
use crate::util::check_path;

pub fn module(lua: &Lua) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;

    t.set(
        "sha256",
        lua.create_function(|_, content: String| {
            let hash = Sha256::digest(content.as_bytes());
            Ok(format!("{hash:x}"))
        })?,
    )?;

    t.set(
        "sha256_file",
        lua.create_function(|lua, path: String| {
            let access = check_path(lua, &path, PathOp::Read)?;

            use std::io::Read;
            let mut reader = access.open_read().map_err(LuaError::external)?;
            let mut hasher = Sha256::new();
            let mut buf = [0u8; 8192];
            loop {
                let n = reader.read(&mut buf).map_err(LuaError::external)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
            Ok(format!("{:x}", hasher.finalize()))
        })?,
    )?;

    Ok(t)
}

#[cfg(test)]
mod tests {
    use mlua::Lua;

    use crate::util::test_eval as eval;

    #[test]
    fn sha256_known_value() {
        let s: String = eval(r#"return std.hash.sha256("hello")"#);
        assert_eq!(
            s,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn sha256_empty_string() {
        let s: String = eval(r#"return std.hash.sha256("")"#);
        assert_eq!(
            s,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_file() {
        let dir = std::env::temp_dir().join("mlua_bat_test_hash");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.txt");
        std::fs::write(&path, "hello").unwrap();
        let path_str = path.to_string_lossy();

        let s: String = eval(&format!(r#"return std.hash.sha256_file("{path_str}")"#));
        assert_eq!(
            s,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sha256_file_nonexistent_returns_error() {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let result: mlua::Result<mlua::Value> = lua
            .load(r#"return std.hash.sha256_file("/tmp/__mlua_bat_nonexistent_hash__")"#)
            .eval();
        assert!(result.is_err());
    }

    #[test]
    fn sha256_file_blocked_by_sandbox() {
        let lua = Lua::new();
        let sandbox_dir = std::env::temp_dir().join("mlua_bat_test_sandbox_hash");
        std::fs::create_dir_all(&sandbox_dir).unwrap();

        let config = crate::config::Config::builder()
            .path_policy(crate::policy::Sandboxed::new([&sandbox_dir]).unwrap())
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        let result: mlua::Result<mlua::Value> = lua
            .load(r#"return std.hash.sha256_file("/etc/hosts")"#)
            .eval();
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&sandbox_dir);
    }
}
