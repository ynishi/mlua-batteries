//! Time and measurement module.
//!
//! ```lua
//! local time = std.time
//! local ts = time.now()       -- epoch seconds (f64)
//! local ms = time.millis()    -- epoch milliseconds (i64)
//! time.sleep(0.5)             -- block for 0.5 seconds
//! local elapsed, result = time.measure(function() return "done" end)
//! ```

use mlua::prelude::*;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::util::with_config;

pub fn module(lua: &Lua) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;

    t.set(
        "now",
        lua.create_function(|_, _: ()| {
            let dur = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(LuaError::external)?;
            Ok(dur.as_secs_f64())
        })?,
    )?;

    t.set(
        "millis",
        lua.create_function(|_, _: ()| {
            let dur = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(LuaError::external)?;
            // as_millis() returns u128. Saturate to i64::MAX (~292 million years).
            let ms: i64 = dur.as_millis().try_into().unwrap_or(i64::MAX);
            Ok(ms)
        })?,
    )?;

    t.set(
        "sleep",
        lua.create_function(|lua, seconds: f64| {
            if !seconds.is_finite() || seconds < 0.0 {
                return Err(LuaError::external(format!(
                    "sleep duration must be a finite non-negative number, got {seconds}"
                )));
            }
            let max_secs = with_config(lua, |c| c.max_sleep_secs)?;
            if seconds > max_secs {
                return Err(LuaError::external(format!(
                    "sleep duration must not exceed {max_secs} seconds"
                )));
            }
            std::thread::sleep(Duration::from_secs_f64(seconds));
            Ok(())
        })?,
    )?;

    t.set(
        "measure",
        lua.create_function(|_, func: LuaFunction| {
            let start = std::time::Instant::now();
            let result: LuaMultiValue = func.call(())?;
            let elapsed = start.elapsed().as_secs_f64();
            let mut ret = vec![LuaValue::Number(elapsed)];
            ret.extend(result);
            Ok(LuaMultiValue::from_vec(ret))
        })?,
    )?;

    Ok(t)
}

#[cfg(test)]
mod tests {
    use mlua::Lua;

    use crate::util::test_eval as eval;

    #[test]
    fn now_returns_positive() {
        let ts: f64 = eval("return std.time.now()");
        assert!(ts > 1_000_000_000.0);
    }

    #[test]
    fn millis_returns_positive() {
        let ms: i64 = eval("return std.time.millis()");
        assert!(ms > 1_000_000_000_000);
    }

    #[test]
    fn now_and_millis_consistent() {
        let diff: f64 = eval(
            r#"
            local sec = std.time.now()
            local ms = std.time.millis()
            return math.abs(sec * 1000 - ms)
        "#,
        );
        assert!(diff < 100.0);
    }

    #[test]
    fn sleep_pauses() {
        let elapsed: f64 = eval(
            r#"
            local before = std.time.now()
            std.time.sleep(0.05)
            return std.time.now() - before
        "#,
        );
        assert!(elapsed >= 0.04);
    }

    #[test]
    fn sleep_zero_is_valid() {
        let ok: bool = eval(
            r#"
            std.time.sleep(0)
            return true
        "#,
        );
        assert!(ok);
    }

    #[test]
    fn sleep_negative_returns_error() {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let result: mlua::Result<mlua::Value> = lua.load("std.time.sleep(-1)").eval();
        assert!(result.is_err());
    }

    #[test]
    fn sleep_nan_returns_error() {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let result: mlua::Result<mlua::Value> = lua.load("std.time.sleep(0/0)").eval();
        assert!(result.is_err());
    }

    #[test]
    fn sleep_exceeding_max_returns_error() {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let result: mlua::Result<mlua::Value> = lua.load("std.time.sleep(86401)").eval();
        assert!(result.is_err());
    }

    #[test]
    fn custom_max_sleep_enforced() {
        let lua = Lua::new();
        let config = crate::config::Config::builder()
            .max_sleep_secs(1.0)
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        let result: mlua::Result<mlua::Value> = lua.load("std.time.sleep(2)").eval();
        assert!(result.is_err());
    }

    #[test]
    fn measure_returns_elapsed_and_result() {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let (elapsed, value): (f64, String) = lua
            .load(
                r#"
                local elapsed, result = std.time.measure(function()
                    std.time.sleep(0.05)
                    return "done"
                end)
                return elapsed, result
            "#,
            )
            .eval()
            .unwrap();
        assert!(elapsed >= 0.04);
        assert_eq!(value, "done");
    }

    #[test]
    fn measure_propagates_error() {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let result: mlua::Result<mlua::Value> = lua
            .load(r#"return std.time.measure(function() error("boom") end)"#)
            .eval();
        assert!(result.is_err());
    }
}
