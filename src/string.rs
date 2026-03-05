//! Extended string operations (Unicode-aware).
//!
//! Complements Lua's built-in `string` library with operations that are
//! either missing or only ASCII-aware in standard Lua.
//!
//! ```lua
//! local str = std.string
//! str.trim("  hello  ")            --> "hello"
//! str.split("a,b,c", ",")          --> {"a", "b", "c"}
//! str.starts_with("hello", "he")   --> true
//! str.replace("abab", "ab", "x")   --> "xab"
//! str.replace_all("abab", "ab", "x") --> "xx"
//! str.upper("café")                --> "CAFÉ"
//! str.pad_start("42", 5, "0")      --> "00042"
//! ```

use mlua::prelude::*;

pub fn module(lua: &Lua) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;

    // ─── Trimming ─────────────────────────────────────────

    t.set(
        "trim",
        lua.create_function(|_, s: String| Ok(s.trim().to_string()))?,
    )?;

    t.set(
        "trim_start",
        lua.create_function(|_, s: String| Ok(s.trim_start().to_string()))?,
    )?;

    t.set(
        "trim_end",
        lua.create_function(|_, s: String| Ok(s.trim_end().to_string()))?,
    )?;

    // ─── Split ────────────────────────────────────────────

    t.set(
        "split",
        lua.create_function(|lua, (s, sep): (String, String)| {
            if sep.is_empty() {
                return Err(LuaError::external(
                    "string.split: separator must not be empty",
                ));
            }
            let table = lua.create_table()?;
            for (i, part) in s.split(&*sep).enumerate() {
                table.set(i + 1, part)?;
            }
            Ok(table)
        })?,
    )?;

    // ─── Predicates ───────────────────────────────────────

    t.set(
        "starts_with",
        lua.create_function(|_, (s, prefix): (String, String)| Ok(s.starts_with(&*prefix)))?,
    )?;

    t.set(
        "ends_with",
        lua.create_function(|_, (s, suffix): (String, String)| Ok(s.ends_with(&*suffix)))?,
    )?;

    t.set(
        "contains",
        lua.create_function(|_, (s, needle): (String, String)| Ok(s.contains(&*needle)))?,
    )?;

    // ─── Replace (non-regex) ──────────────────────────────

    t.set(
        "replace",
        lua.create_function(|_, (s, from, to): (String, String, String)| {
            Ok(s.replacen(&*from, &to, 1))
        })?,
    )?;

    t.set(
        "replace_all",
        lua.create_function(|_, (s, from, to): (String, String, String)| {
            Ok(s.replace(&*from, &to))
        })?,
    )?;

    // ─── Padding ──────────────────────────────────────────

    t.set(
        "pad_start",
        lua.create_function(|_, (s, width, fill): (String, usize, Option<String>)| {
            let fill_char = parse_fill_char(&fill)?;
            let char_count = s.chars().count();
            if char_count >= width {
                return Ok(s);
            }
            let padding: String = std::iter::repeat(fill_char)
                .take(width - char_count)
                .collect();
            Ok(format!("{padding}{s}"))
        })?,
    )?;

    t.set(
        "pad_end",
        lua.create_function(|_, (s, width, fill): (String, usize, Option<String>)| {
            let fill_char = parse_fill_char(&fill)?;
            let char_count = s.chars().count();
            if char_count >= width {
                return Ok(s);
            }
            let padding: String = std::iter::repeat(fill_char)
                .take(width - char_count)
                .collect();
            Ok(format!("{s}{padding}"))
        })?,
    )?;

    // ─── Truncate ─────────────────────────────────────────

    t.set(
        "truncate",
        lua.create_function(|_, (s, max_len, suffix): (String, usize, Option<String>)| {
            let suffix = suffix.unwrap_or_default();
            let char_count = s.chars().count();
            if char_count <= max_len {
                return Ok(s);
            }
            let suffix_len = suffix.chars().count();
            if max_len <= suffix_len {
                return Ok(suffix.chars().take(max_len).collect());
            }
            let keep = max_len - suffix_len;
            let truncated: String = s.chars().take(keep).collect();
            Ok(format!("{truncated}{suffix}"))
        })?,
    )?;

    // ─── Unicode-aware case conversion ────────────────────

    t.set(
        "upper",
        lua.create_function(|_, s: String| Ok(s.to_uppercase()))?,
    )?;

    t.set(
        "lower",
        lua.create_function(|_, s: String| Ok(s.to_lowercase()))?,
    )?;

    // ─── Unicode utilities ────────────────────────────────

    t.set(
        "chars",
        lua.create_function(|lua, s: String| {
            let table = lua.create_table()?;
            for (i, ch) in s.chars().enumerate() {
                let mut buf = [0u8; 4];
                table.set(i + 1, &*ch.encode_utf8(&mut buf))?;
            }
            Ok(table)
        })?,
    )?;

    t.set(
        "char_count",
        lua.create_function(|_, s: String| Ok(s.chars().count()))?,
    )?;

    t.set(
        "reverse",
        lua.create_function(|_, s: String| Ok(s.chars().rev().collect::<String>()))?,
    )?;

    Ok(t)
}

fn parse_fill_char(fill: &Option<String>) -> LuaResult<char> {
    match fill {
        None => Ok(' '),
        Some(s) => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Ok(c),
                _ => Err(LuaError::external("fill must be a single character")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::util::test_eval as eval;

    // ─── trim ─────────────────────────────────────────────

    #[test]
    fn trim_whitespace() {
        let s: String = eval(r#"return std.string.trim("  hello  ")"#);
        assert_eq!(s, "hello");
    }

    #[test]
    fn trim_start_whitespace() {
        let s: String = eval(r#"return std.string.trim_start("  hello  ")"#);
        assert_eq!(s, "hello  ");
    }

    #[test]
    fn trim_end_whitespace() {
        let s: String = eval(r#"return std.string.trim_end("  hello  ")"#);
        assert_eq!(s, "  hello");
    }

    #[test]
    fn trim_empty_string() {
        let s: String = eval(r#"return std.string.trim("")"#);
        assert_eq!(s, "");
    }

    #[test]
    fn trim_no_whitespace() {
        let s: String = eval(r#"return std.string.trim("hello")"#);
        assert_eq!(s, "hello");
    }

    // ─── split ────────────────────────────────────────────

    #[test]
    fn split_by_comma() {
        let s: String = eval(
            r#"
            local parts = std.string.split("a,b,c", ",")
            return parts[1] .. "|" .. parts[2] .. "|" .. parts[3]
        "#,
        );
        assert_eq!(s, "a|b|c");
    }

    #[test]
    fn split_no_match() {
        let n: i64 = eval(
            r#"
            local parts = std.string.split("hello", ",")
            return #parts
        "#,
        );
        assert_eq!(n, 1);
    }

    #[test]
    fn split_empty_parts() {
        let n: i64 = eval(
            r#"
            local parts = std.string.split(",a,,b,", ",")
            return #parts
        "#,
        );
        assert_eq!(n, 5);
    }

    #[test]
    fn split_empty_separator_returns_error() {
        let lua = mlua::Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let result: mlua::Result<mlua::Value> =
            lua.load(r#"return std.string.split("abc", "")"#).eval();
        assert!(result.is_err());
    }

    #[test]
    fn split_multi_char_separator() {
        let s: String = eval(
            r#"
            local parts = std.string.split("a::b::c", "::")
            return parts[1] .. "|" .. parts[2] .. "|" .. parts[3]
        "#,
        );
        assert_eq!(s, "a|b|c");
    }

    // ─── predicates ───────────────────────────────────────

    #[test]
    fn starts_with_true() {
        let b: bool = eval(r#"return std.string.starts_with("hello world", "hello")"#);
        assert!(b);
    }

    #[test]
    fn starts_with_false() {
        let b: bool = eval(r#"return std.string.starts_with("hello world", "world")"#);
        assert!(!b);
    }

    #[test]
    fn ends_with_true() {
        let b: bool = eval(r#"return std.string.ends_with("hello world", "world")"#);
        assert!(b);
    }

    #[test]
    fn ends_with_false() {
        let b: bool = eval(r#"return std.string.ends_with("hello world", "hello")"#);
        assert!(!b);
    }

    #[test]
    fn contains_true() {
        let b: bool = eval(r#"return std.string.contains("hello world", "lo wo")"#);
        assert!(b);
    }

    #[test]
    fn contains_false() {
        let b: bool = eval(r#"return std.string.contains("hello world", "xyz")"#);
        assert!(!b);
    }

    // ─── replace ──────────────────────────────────────────

    #[test]
    fn replace_first_only() {
        let s: String = eval(r#"return std.string.replace("abab", "ab", "x")"#);
        assert_eq!(s, "xab");
    }

    #[test]
    fn replace_all_occurrences() {
        let s: String = eval(r#"return std.string.replace_all("abab", "ab", "x")"#);
        assert_eq!(s, "xx");
    }

    #[test]
    fn replace_no_match() {
        let s: String = eval(r#"return std.string.replace("hello", "xyz", "!")"#);
        assert_eq!(s, "hello");
    }

    // ─── pad ──────────────────────────────────────────────

    #[test]
    fn pad_start_with_zeros() {
        let s: String = eval(r#"return std.string.pad_start("42", 5, "0")"#);
        assert_eq!(s, "00042");
    }

    #[test]
    fn pad_start_default_space() {
        let s: String = eval(r#"return std.string.pad_start("hi", 5)"#);
        assert_eq!(s, "   hi");
    }

    #[test]
    fn pad_start_already_long() {
        let s: String = eval(r#"return std.string.pad_start("hello", 3, "x")"#);
        assert_eq!(s, "hello");
    }

    #[test]
    fn pad_end_with_dots() {
        let s: String = eval(r#"return std.string.pad_end("hi", 5, ".")"#);
        assert_eq!(s, "hi...");
    }

    #[test]
    fn pad_end_default_space() {
        let s: String = eval(r#"return std.string.pad_end("hi", 5)"#);
        assert_eq!(s, "hi   ");
    }

    #[test]
    fn pad_fill_multi_char_returns_error() {
        let lua = mlua::Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let result: mlua::Result<mlua::Value> = lua
            .load(r#"return std.string.pad_start("x", 5, "ab")"#)
            .eval();
        assert!(result.is_err());
    }

    // ─── truncate ─────────────────────────────────────────

    #[test]
    fn truncate_with_ellipsis() {
        let s: String = eval(r#"return std.string.truncate("hello world", 8, "...")"#);
        assert_eq!(s, "hello...");
    }

    #[test]
    fn truncate_no_suffix() {
        let s: String = eval(r#"return std.string.truncate("hello world", 5)"#);
        assert_eq!(s, "hello");
    }

    #[test]
    fn truncate_already_short() {
        let s: String = eval(r#"return std.string.truncate("hi", 10, "...")"#);
        assert_eq!(s, "hi");
    }

    #[test]
    fn truncate_max_equals_suffix_len() {
        let s: String = eval(r#"return std.string.truncate("hello", 3, "...")"#);
        assert_eq!(s, "...");
    }

    #[test]
    fn truncate_max_less_than_suffix_len() {
        let s: String = eval(r#"return std.string.truncate("hello", 2, "...")"#);
        assert_eq!(s, "..");
    }

    // ─── Unicode-aware case conversion ────────────────────

    #[test]
    fn upper_unicode() {
        let s: String = eval(r#"return std.string.upper("café")"#);
        assert_eq!(s, "CAFÉ");
    }

    #[test]
    fn lower_unicode() {
        let s: String = eval(r#"return std.string.lower("CAFÉ")"#);
        assert_eq!(s, "café");
    }

    #[test]
    fn upper_german_eszett() {
        let s: String = eval(r#"return std.string.upper("straße")"#);
        assert_eq!(s, "STRASSE");
    }

    // ─── Unicode utilities ────────────────────────────────

    #[test]
    fn chars_ascii() {
        let s: String = eval(
            r#"
            local cs = std.string.chars("abc")
            return cs[1] .. cs[2] .. cs[3]
        "#,
        );
        assert_eq!(s, "abc");
    }

    #[test]
    fn chars_multibyte() {
        let n: i64 = eval(r#"return #std.string.chars("café")"#);
        assert_eq!(n, 4);
    }

    #[test]
    fn char_count_ascii() {
        let n: i64 = eval(r#"return std.string.char_count("hello")"#);
        assert_eq!(n, 5);
    }

    #[test]
    fn char_count_multibyte() {
        let n: i64 = eval(r#"return std.string.char_count("café")"#);
        assert_eq!(n, 4);
    }

    #[test]
    fn char_count_emoji() {
        let n: i64 = eval(r#"return std.string.char_count("👋🌍")"#);
        assert_eq!(n, 2);
    }

    #[test]
    fn reverse_ascii() {
        let s: String = eval(r#"return std.string.reverse("hello")"#);
        assert_eq!(s, "olleh");
    }

    #[test]
    fn reverse_unicode() {
        let s: String = eval(r#"return std.string.reverse("café")"#);
        assert_eq!(s, "éfac");
    }

    #[test]
    fn reverse_empty() {
        let s: String = eval(r#"return std.string.reverse("")"#);
        assert_eq!(s, "");
    }
}
