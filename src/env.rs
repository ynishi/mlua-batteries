//! Environment variable module.
//!
//! All access is subject to the active
//! [`EnvPolicy`](crate::policy::EnvPolicy).
//!
//! ```lua
//! local env = std.env
//! local home = env.home()
//! local val  = env.get("PATH")
//! local val2 = env.get_or("MISSING", "default")
//! env.set("KEY", "value")  -- overlay only, safe
//! ```

use mlua::prelude::*;
use std::collections::HashMap;

use crate::util::{check_env_get, check_env_set};

/// Lua-side environment variable overrides.
///
/// Values set via `env.set()` are stored here instead of the OS
/// environment, avoiding `unsafe` calls to `std::env::set_var`.
/// `env.get()` checks this overlay first, falling back to the
/// real OS environment.
struct EnvOverrides(HashMap<String, String>);

pub fn module(lua: &Lua) -> LuaResult<LuaTable> {
    if lua.app_data_ref::<EnvOverrides>().is_none() {
        lua.set_app_data(EnvOverrides(HashMap::new()));
    }

    let t = lua.create_table()?;

    t.set(
        "get",
        lua.create_function(|lua, key: String| {
            check_env_get(lua, &key)?;
            if let Some(ov) = lua.app_data_ref::<EnvOverrides>() {
                if let Some(val) = ov.0.get(&key) {
                    return Ok(Some(val.clone()));
                }
            }
            Ok(std::env::var(&key).ok())
        })?,
    )?;

    t.set(
        "get_or",
        lua.create_function(|lua, (key, default): (String, String)| {
            check_env_get(lua, &key)?;
            if let Some(ov) = lua.app_data_ref::<EnvOverrides>() {
                if let Some(val) = ov.0.get(&key) {
                    return Ok(val.clone());
                }
            }
            Ok(std::env::var(&key).unwrap_or(default))
        })?,
    )?;

    t.set(
        "set",
        lua.create_function(|lua, (key, value): (String, String)| {
            check_env_set(lua, &key)?;
            let mut ov = lua
                .app_data_mut::<EnvOverrides>()
                .ok_or_else(|| LuaError::external("env overlay not initialized"))?;
            ov.0.insert(key, value);
            Ok(())
        })?,
    )?;

    t.set(
        "home",
        lua.create_function(|lua, _: ()| {
            // Try HOME first (primary on Unix).
            let home_allowed = check_env_get(lua, "HOME").is_ok();

            // USERPROFILE fallback (primary on Windows, sometimes set on Unix).
            // Each variable requires its own policy check to prevent bypass:
            // an operator who allows HOME but denies USERPROFILE must not
            // have USERPROFILE read through the fallback path.
            let userprofile_allowed = check_env_get(lua, "USERPROFILE").is_ok();

            if !home_allowed && !userprofile_allowed {
                // Neither variable is allowed — propagate the HOME check error
                // (more informative than a generic denial).
                check_env_get(lua, "HOME")?;
            }

            // Check overlay first
            if let Some(ov) = lua.app_data_ref::<EnvOverrides>() {
                if home_allowed {
                    if let Some(val) = ov.0.get("HOME") {
                        return Ok(Some(val.clone()));
                    }
                }
                if userprofile_allowed {
                    if let Some(val) = ov.0.get("USERPROFILE") {
                        return Ok(Some(val.clone()));
                    }
                }
            }

            // Check OS environment
            if home_allowed {
                if let Ok(val) = std::env::var("HOME") {
                    return Ok(Some(val));
                }
            }
            if userprofile_allowed {
                if let Ok(val) = std::env::var("USERPROFILE") {
                    return Ok(Some(val));
                }
            }

            Ok(None)
        })?,
    )?;

    Ok(t)
}

#[cfg(test)]
mod tests {
    use crate::util::test_eval as eval;

    #[test]
    fn get_existing_var() {
        let s: String = eval(
            r#"
            return type(std.env.get("PATH"))
        "#,
        );
        assert_eq!(s, "string");
    }

    #[test]
    fn get_missing_var_returns_nil() {
        let b: bool = eval(
            r#"
            return std.env.get("__MLUA_STD_DOES_NOT_EXIST__") == nil
        "#,
        );
        assert!(b);
    }

    #[test]
    fn get_or_returns_default() {
        let s: String = eval(
            r#"
            return std.env.get_or("__MLUA_STD_MISSING__", "fallback")
        "#,
        );
        assert_eq!(s, "fallback");
    }

    #[test]
    fn home_returns_string() {
        let s: String = eval(
            r#"
            local h = std.env.home()
            return h ~= nil and "ok" or "nil"
        "#,
        );
        assert_eq!(s, "ok");
    }

    #[test]
    fn set_and_get_roundtrip() {
        let s: String = eval(
            r#"
            std.env.set("__MLUA_STD_TEST__", "test_value")
            return std.env.get("__MLUA_STD_TEST__")
        "#,
        );
        assert_eq!(s, "test_value");
    }

    #[test]
    fn set_overrides_os_var() {
        let s: String = eval(
            r#"
            std.env.set("PATH", "overridden")
            return std.env.get("PATH")
        "#,
        );
        assert_eq!(s, "overridden");
    }

    #[test]
    fn home_reflects_overlay() {
        let s: String = eval(
            r#"
            std.env.set("HOME", "/custom/home")
            return std.env.home()
        "#,
        );
        assert_eq!(s, "/custom/home");
    }

    #[test]
    fn set_then_get_or_returns_overlay() {
        let s: String = eval(
            r#"
            std.env.set("__MLUA_STD_OVERLAY__", "from_overlay")
            return std.env.get_or("__MLUA_STD_OVERLAY__", "default")
        "#,
        );
        assert_eq!(s, "from_overlay");
    }

    // ─── H-2: USERPROFILE policy check tests ─────────

    #[test]
    fn home_blocked_when_both_vars_denied() {
        use crate::policy::EnvAllowList;

        let lua = mlua::Lua::new();
        // Allow only PATH — neither HOME nor USERPROFILE is permitted
        let config = crate::config::Config::builder()
            .env_policy(EnvAllowList::new(["PATH"]))
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        let result: mlua::Result<mlua::Value> = lua.load(r#"return std.env.home()"#).eval();
        assert!(
            result.is_err(),
            "home() should fail when both HOME and USERPROFILE are denied"
        );
    }

    #[test]
    fn home_allowed_when_home_permitted() {
        use crate::policy::EnvAllowList;

        let lua = mlua::Lua::new();
        let config = crate::config::Config::builder()
            .env_policy(EnvAllowList::new(["HOME", "USERPROFILE"]))
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        // HOME is in allow list — should succeed
        let result: mlua::Result<mlua::Value> = lua.load(r#"return std.env.home()"#).eval();
        assert!(result.is_ok(), "home() should succeed when HOME is allowed");
    }

    #[test]
    fn home_works_with_only_home_allowed() {
        use crate::policy::EnvAllowList;

        let lua = mlua::Lua::new();
        // Allow HOME only — USERPROFILE is denied by policy.
        // home() should still return a value (from OS HOME) without
        // attempting to read the denied USERPROFILE.
        let config = crate::config::Config::builder()
            .env_policy(EnvAllowList::new(["HOME"]))
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        let result: mlua::Result<mlua::Value> = lua.load(r#"return std.env.home()"#).eval();
        assert!(
            result.is_ok(),
            "home() should succeed when HOME is allowed even if USERPROFILE is denied"
        );
    }
}
