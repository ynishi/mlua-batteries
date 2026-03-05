//! Regular expression module backed by Rust's `regex` crate.
//!
//! Provides full regular expression support, replacing Lua's limited
//! pattern matching with Rust's high-performance regex engine.
//!
//! ```lua
//! local re = std.regex
//! re.is_match("hello123", "\\d+")                    --> true
//! re.find("hello 123 world", "\\d+")                 --> {start=7, stop=9, text="123"}
//! re.find_all("a1b2c3", "\\d")                       --> {{...}, {...}, {...}}
//! re.captures("2024-01-15", "(\\d{4})-(\\d{2})-(\\d{2})")
//!     --> {"2024-01-15", "2024", "01", "15"}
//! re.replace("hello 123", "\\d+", "NUM")             --> "hello NUM"
//! re.replace_all("a1b2", "\\d", "X")                 --> "aXbX"
//! re.split("a, b, c", ",\\s*")                       --> {"a", "b", "c"}
//! ```

use mlua::prelude::*;
use regex::Regex;

fn compile(pattern: &str) -> LuaResult<Regex> {
    Regex::new(pattern).map_err(|e| LuaError::external(format!("regex: invalid pattern: {e}")))
}

pub fn module(lua: &Lua) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;

    // ─── is_match ─────────────────────────────────────────

    t.set(
        "is_match",
        lua.create_function(|_, (s, pattern): (String, String)| {
            let re = compile(&pattern)?;
            Ok(re.is_match(&s))
        })?,
    )?;

    // ─── find ─────────────────────────────────────────────

    t.set(
        "find",
        lua.create_function(|lua, (s, pattern): (String, String)| {
            let re = compile(&pattern)?;
            match re.find(&s) {
                Some(m) => {
                    let table = lua.create_table()?;
                    table.set("start", m.start() + 1)?; // 1-based
                    table.set("stop", m.end())?; // inclusive end in Lua convention
                    table.set("text", &s[m.start()..m.end()])?;
                    Ok(LuaValue::Table(table))
                }
                None => Ok(LuaValue::Nil),
            }
        })?,
    )?;

    // ─── find_all ─────────────────────────────────────────

    t.set(
        "find_all",
        lua.create_function(|lua, (s, pattern): (String, String)| {
            let re = compile(&pattern)?;
            let results = lua.create_table()?;
            for (i, m) in re.find_iter(&s).enumerate() {
                let entry = lua.create_table()?;
                entry.set("start", m.start() + 1)?;
                entry.set("stop", m.end())?;
                entry.set("text", &s[m.start()..m.end()])?;
                results.set(i + 1, entry)?;
            }
            Ok(results)
        })?,
    )?;

    // ─── captures ─────────────────────────────────────────

    t.set(
        "captures",
        lua.create_function(|lua, (s, pattern): (String, String)| {
            let re = compile(&pattern)?;
            match re.captures(&s) {
                Some(caps) => {
                    let table = lua.create_table()?;
                    // Index 0 is the full match, stored at Lua index 1
                    for i in 0..caps.len() {
                        if let Some(m) = caps.get(i) {
                            table.set(i + 1, m.as_str())?;
                        }
                    }
                    Ok(LuaValue::Table(table))
                }
                None => Ok(LuaValue::Nil),
            }
        })?,
    )?;

    // ─── replace ──────────────────────────────────────────

    t.set(
        "replace",
        lua.create_function(|_, (s, pattern, replacement): (String, String, String)| {
            let re = compile(&pattern)?;
            Ok(re.replacen(&s, 1, replacement.as_str()).into_owned())
        })?,
    )?;

    // ─── replace_all ──────────────────────────────────────

    t.set(
        "replace_all",
        lua.create_function(|_, (s, pattern, replacement): (String, String, String)| {
            let re = compile(&pattern)?;
            Ok(re.replace_all(&s, replacement.as_str()).into_owned())
        })?,
    )?;

    // ─── split ────────────────────────────────────────────

    t.set(
        "split",
        lua.create_function(|lua, (s, pattern): (String, String)| {
            let re = compile(&pattern)?;
            let table = lua.create_table()?;
            for (i, part) in re.split(&s).enumerate() {
                table.set(i + 1, part)?;
            }
            Ok(table)
        })?,
    )?;

    Ok(t)
}

#[cfg(test)]
mod tests {
    use crate::util::test_eval as eval;

    // ─── is_match ─────────────────────────────────────────

    #[test]
    fn is_match_true() {
        let b: bool = eval(r#"return std.regex.is_match("hello123", "\\d+")"#);
        assert!(b);
    }

    #[test]
    fn is_match_false() {
        let b: bool = eval(r#"return std.regex.is_match("hello", "\\d+")"#);
        assert!(!b);
    }

    #[test]
    fn is_match_invalid_pattern_returns_error() {
        let lua = mlua::Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let result: mlua::Result<mlua::Value> = lua
            .load(r#"return std.regex.is_match("hello", "[")"#)
            .eval();
        assert!(result.is_err());
    }

    // ─── find ─────────────────────────────────────────────

    #[test]
    fn find_returns_match_info() {
        let s: String = eval(
            r#"
            local m = std.regex.find("hello 123 world", "\\d+")
            return m.start .. "," .. m.stop .. "," .. m.text
        "#,
        );
        assert_eq!(s, "7,9,123");
    }

    #[test]
    fn find_no_match_returns_nil() {
        let b: bool = eval(
            r#"
            return std.regex.find("hello", "\\d+") == nil
        "#,
        );
        assert!(b);
    }

    // ─── find_all ─────────────────────────────────────────

    #[test]
    fn find_all_multiple_matches() {
        let s: String = eval(
            r#"
            local ms = std.regex.find_all("a1b2c3", "\\d")
            return ms[1].text .. ms[2].text .. ms[3].text
        "#,
        );
        assert_eq!(s, "123");
    }

    #[test]
    fn find_all_no_match_returns_empty() {
        let n: i64 = eval(
            r#"
            return #std.regex.find_all("hello", "\\d")
        "#,
        );
        assert_eq!(n, 0);
    }

    #[test]
    fn find_all_positions_are_1_based() {
        let s: String = eval(
            r#"
            local ms = std.regex.find_all("abc", "[a-c]")
            return ms[1].start .. "," .. ms[2].start .. "," .. ms[3].start
        "#,
        );
        assert_eq!(s, "1,2,3");
    }

    // ─── captures ─────────────────────────────────────────

    #[test]
    fn captures_groups() {
        let s: String = eval(
            r#"
            local caps = std.regex.captures("2024-01-15", "(\\d{4})-(\\d{2})-(\\d{2})")
            return caps[1] .. "|" .. caps[2] .. "|" .. caps[3] .. "|" .. caps[4]
        "#,
        );
        assert_eq!(s, "2024-01-15|2024|01|15");
    }

    #[test]
    fn captures_no_match_returns_nil() {
        let b: bool = eval(
            r#"
            return std.regex.captures("hello", "(\\d+)") == nil
        "#,
        );
        assert!(b);
    }

    #[test]
    fn captures_no_groups() {
        let s: String = eval(
            r#"
            local caps = std.regex.captures("hello", "\\w+")
            return caps[1]
        "#,
        );
        assert_eq!(s, "hello");
    }

    // ─── replace ──────────────────────────────────────────

    #[test]
    fn replace_first_only() {
        let s: String = eval(r#"return std.regex.replace("a1b2c3", "\\d", "X")"#);
        assert_eq!(s, "aXb2c3");
    }

    #[test]
    fn replace_all_occurrences() {
        let s: String = eval(r#"return std.regex.replace_all("a1b2c3", "\\d", "X")"#);
        assert_eq!(s, "aXbXcX");
    }

    #[test]
    fn replace_with_capture_ref() {
        let s: String = eval(r#"return std.regex.replace_all("foo bar", "(\\w+)", "[$1]")"#);
        assert_eq!(s, "[foo] [bar]");
    }

    #[test]
    fn replace_no_match_unchanged() {
        let s: String = eval(r#"return std.regex.replace("hello", "\\d+", "X")"#);
        assert_eq!(s, "hello");
    }

    // ─── split ────────────────────────────────────────────

    #[test]
    fn split_by_regex() {
        let s: String = eval(
            r#"
            local parts = std.regex.split("a, b,  c", ",\\s*")
            return parts[1] .. "|" .. parts[2] .. "|" .. parts[3]
        "#,
        );
        assert_eq!(s, "a|b|c");
    }

    #[test]
    fn split_no_match_single_element() {
        let n: i64 = eval(r#"return #std.regex.split("hello", "\\d+")"#);
        assert_eq!(n, 1);
    }

    #[test]
    fn split_consecutive_separators() {
        let n: i64 = eval(r#"return #std.regex.split("a,,b", ",")"#);
        assert_eq!(n, 3);
    }
}
