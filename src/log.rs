//! Structured logging bridge to Rust's `log` crate.
//!
//! Lua log calls are forwarded to the host application's log subscriber
//! with target `"lua"`, enabling unified log collection.
//!
//! ```lua
//! local log = std.log
//! log.debug("cache hit")
//! log.info("processing", {user = "john", count = 42})
//! log.warn("deprecated API")
//! log.error("request failed", {code = 500})
//! log.is_enabled("debug")  --> true/false
//! ```

use mlua::prelude::*;

const TARGET: &str = "lua";

pub fn module(lua: &Lua) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;

    t.set(
        "debug",
        lua.create_function(|_, (msg, data): (String, Option<LuaTable>)| {
            let formatted = format_log_message(&msg, data.as_ref())?;
            ::log::debug!(target: TARGET, "{}", formatted);
            Ok(())
        })?,
    )?;

    t.set(
        "info",
        lua.create_function(|_, (msg, data): (String, Option<LuaTable>)| {
            let formatted = format_log_message(&msg, data.as_ref())?;
            ::log::info!(target: TARGET, "{}", formatted);
            Ok(())
        })?,
    )?;

    t.set(
        "warn",
        lua.create_function(|_, (msg, data): (String, Option<LuaTable>)| {
            let formatted = format_log_message(&msg, data.as_ref())?;
            ::log::warn!(target: TARGET, "{}", formatted);
            Ok(())
        })?,
    )?;

    t.set(
        "error",
        lua.create_function(|_, (msg, data): (String, Option<LuaTable>)| {
            let formatted = format_log_message(&msg, data.as_ref())?;
            ::log::error!(target: TARGET, "{}", formatted);
            Ok(())
        })?,
    )?;

    t.set(
        "is_enabled",
        lua.create_function(|_, level: String| {
            let level = parse_level(&level)?;
            Ok(::log::log_enabled!(target: TARGET, level))
        })?,
    )?;

    Ok(t)
}

fn parse_level(s: &str) -> LuaResult<::log::Level> {
    match s {
        "debug" => Ok(::log::Level::Debug),
        "info" => Ok(::log::Level::Info),
        "warn" => Ok(::log::Level::Warn),
        "error" => Ok(::log::Level::Error),
        "trace" => Ok(::log::Level::Trace),
        _ => Err(LuaError::external(format!(
            "log: unknown level \"{s}\", expected debug|info|warn|error|trace"
        ))),
    }
}

fn format_log_message(msg: &str, data: Option<&LuaTable>) -> LuaResult<String> {
    let Some(table) = data else {
        return Ok(msg.to_string());
    };
    let mut buf = msg.to_string();
    for pair in table.pairs::<String, LuaValue>() {
        let (k, v) = pair?;
        buf.push(' ');
        buf.push_str(&k);
        buf.push('=');
        buf.push_str(&format_value(&v));
    }
    Ok(buf)
}

fn format_value(v: &LuaValue) -> String {
    match v {
        LuaValue::Nil => "nil".to_string(),
        LuaValue::Boolean(b) => b.to_string(),
        LuaValue::Integer(i) => i.to_string(),
        LuaValue::Number(n) => n.to_string(),
        LuaValue::String(s) => s.to_string_lossy(),
        other => format!("<{}>", other.type_name()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use mlua::prelude::LuaValue;

    use crate::util::test_eval as eval;

    struct CapturingLogger;
    static CAPTURED: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

    fn captured() -> &'static Mutex<Vec<String>> {
        CAPTURED.get_or_init(|| Mutex::new(Vec::new()))
    }

    impl ::log::Log for CapturingLogger {
        fn enabled(&self, meta: &::log::Metadata) -> bool {
            meta.target() == "lua"
        }

        fn log(&self, record: &::log::Record) {
            if record.target() == "lua" {
                if let Ok(mut logs) = captured().lock() {
                    logs.push(format!("[{}] {}", record.level(), record.args()));
                }
            }
        }

        fn flush(&self) {}
    }

    fn init_logger() {
        static INIT: OnceLock<()> = OnceLock::new();
        INIT.get_or_init(|| {
            let _ = ::log::set_logger(&CapturingLogger);
            ::log::set_max_level(::log::LevelFilter::Trace);
        });
    }

    fn drain_captured() -> Vec<String> {
        captured().lock().unwrap().drain(..).collect()
    }

    // All capture-based tests in one function to avoid parallel race on the global logger.
    #[test]
    fn log_levels_and_structured_data() {
        init_logger();
        drain_captured();

        eval::<LuaValue>(
            r#"
            std.log.debug("d")
            std.log.info("hello")
            std.log.info("req", {method = "GET"})
            std.log.warn("w")
            std.log.error("e")
        "#,
        );
        let logs = drain_captured();
        assert!(
            logs.iter().any(|l| l.contains("[DEBUG]")),
            "missing DEBUG, got: {logs:?}"
        );
        assert!(
            logs.iter().any(|l| l.contains("[INFO] hello")),
            "missing INFO hello, got: {logs:?}"
        );
        assert!(
            logs.iter().any(|l| l.contains("method=GET")),
            "missing structured data, got: {logs:?}"
        );
        assert!(
            logs.iter().any(|l| l.contains("[WARN]")),
            "missing WARN, got: {logs:?}"
        );
        assert!(
            logs.iter().any(|l| l.contains("[ERROR]")),
            "missing ERROR, got: {logs:?}"
        );
    }

    #[test]
    fn is_enabled_returns_bool() {
        init_logger();
        let b: bool = eval(r#"return std.log.is_enabled("info")"#);
        assert!(b);
    }

    #[test]
    fn invalid_level_returns_error() {
        let lua = mlua::Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let result: mlua::Result<mlua::Value> = lua
            .load(r#"std.log.info("ok"); std.log.is_enabled("bad")"#)
            .eval();
        assert!(result.is_err());
    }
}
