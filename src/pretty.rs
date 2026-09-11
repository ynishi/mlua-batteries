//! Deterministic value dump for debugging, snapshots and test output.
//!
//! ```lua
//! local pretty = std.pretty
//! print(pretty.dump({ b = 2, a = { 1, 2 }, f = print }))
//! -- {
//! --   a = {
//! --     1,
//! --     2
//! --   },
//! --   b = 2,
//! --   f = <function>
//! -- }
//! print(pretty.dump({ 1, { x = true } }, { indent = 0 }))
//! -- { 1, { x = true } }
//! ```
//!
//! `json.encode_pretty` is the right tool when the value is JSON: it
//! fails on anything JSON cannot carry.  `dump` is the tolerant sibling:
//! it accepts every Lua value, never raises, and its output is the same
//! for the same value every time, which is what a snapshot test or a diff
//! between two runs needs.
//!
//! # Output
//!
//! Lua table-constructor syntax, the shape `inspect.lua` and Penlight's
//! `pretty.write` made familiar:
//!
//! - the array part `1..#t` first, then the remaining keys — string keys
//!   in byte order (bare when they are identifiers, `["…"]` otherwise),
//!   then integers, floats and booleans in order, then anything else;
//! - functions, userdata, threads and coroutines as `<function>`,
//!   `<userdata>`, `<thread>` — no addresses, so the output is stable
//!   across runs; a `__tostring` metamethod on a table or userdata wins
//!   and its result is shown as `<…>`;
//! - the `json.null` sentinel as `null`;
//! - a table already open further up the path as `<cycle>` (a table
//!   reached twice by different paths is simply printed twice);
//! - a table below the `depth` limit as `{...}`;
//! - strings `%q`-style: `"`, `\`, newline, tab and control bytes are
//!   escaped, UTF-8 is left as it is; floats print in the shortest form
//!   that round-trips, keeping a `.0` so `3.0` stays a float.
//!
//! Key order does not depend on `serde_json`'s map type or on Lua's hash
//! seed; the sort is done here.  Pass `sort_keys = false` to take Lua's
//! `pairs` order instead when only speed matters.
//!
//! # Options
//!
//! | Key | Default | Meaning |
//! |-----|---------|---------|
//! | `indent` | `2` | Spaces per level; `0` prints everything on one line |
//! | `depth` | unlimited | Tables nested deeper than this print as `{...}` |
//! | `sort_keys` | `true` | Sort the non-array keys as described above |

use std::fmt::Write as _;

use mlua::prelude::*;

/// Rendering options, parsed once per `dump` call.
struct Options {
    indent: usize,
    depth: Option<usize>,
    sort_keys: bool,
}

impl Options {
    fn from_lua(opts: Option<LuaTable>) -> LuaResult<Self> {
        let mut o = Options {
            indent: 2,
            depth: None,
            sort_keys: true,
        };
        let Some(t) = opts else {
            return Ok(o);
        };
        if let Some(n) = t.get::<Option<usize>>("indent")? {
            o.indent = n;
        }
        if let Some(n) = t.get::<Option<usize>>("depth")? {
            o.depth = Some(n);
        }
        if let Some(b) = t.get::<Option<bool>>("sort_keys")? {
            o.sort_keys = b;
        }
        Ok(o)
    }
}

pub fn module(lua: &Lua) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;

    t.set(
        "dump",
        lua.create_function(|_, (value, opts): (LuaValue, Option<LuaTable>)| {
            let opts = Options::from_lua(opts)?;
            let mut out = String::new();
            let mut path: Vec<*const std::ffi::c_void> = Vec::new();
            write_value(&mut out, &value, &opts, 0, &mut path)?;
            Ok(out)
        })?,
    )?;

    Ok(t)
}

/// Render `value` at nesting `level` (0 = top).  `path` holds the tables
/// currently open above this one, for cycle detection.
fn write_value(
    out: &mut String,
    value: &LuaValue,
    opts: &Options,
    level: usize,
    path: &mut Vec<*const std::ffi::c_void>,
) -> LuaResult<()> {
    match value {
        LuaValue::Nil => out.push_str("nil"),
        LuaValue::Boolean(b) => out.push_str(if *b { "true" } else { "false" }),
        LuaValue::Integer(i) => write!(out, "{i}").unwrap(),
        LuaValue::Number(n) => write_number(out, *n),
        LuaValue::String(s) => write_string(out, &s.as_bytes()),
        LuaValue::LightUserData(u) if u.0.is_null() => out.push_str("null"),
        LuaValue::LightUserData(_) => out.push_str("<lightuserdata>"),
        LuaValue::Function(_) => out.push_str("<function>"),
        LuaValue::Thread(_) => out.push_str("<thread>"),
        LuaValue::UserData(_) => match custom_tostring(value)? {
            Some(s) => write_tagged(out, &s),
            None => out.push_str("<userdata>"),
        },
        LuaValue::Table(t) => {
            if let Some(s) = custom_tostring(value)? {
                write_tagged(out, &s);
                return Ok(());
            }
            write_table(out, t, opts, level, path)?;
        }
        LuaValue::Error(e) => write_tagged(out, &format!("error: {e}")),
        other => write_tagged(out, other.type_name()),
    }
    Ok(())
}

/// The result of a `__tostring` metamethod, when the value has one.
fn custom_tostring(value: &LuaValue) -> LuaResult<Option<String>> {
    let has_tostring = match value {
        LuaValue::Table(t) => match t.metatable() {
            Some(mt) => matches!(mt.raw_get::<LuaValue>("__tostring")?, LuaValue::Function(_)),
            None => false,
        },
        LuaValue::UserData(ud) => match ud.metatable() {
            Ok(mt) => matches!(mt.get::<LuaValue>("__tostring")?, LuaValue::Function(_)),
            Err(_) => false,
        },
        _ => false,
    };
    if !has_tostring {
        return Ok(None);
    }
    // `Value::to_string` goes through luaL_tolstring, so the metamethod
    // runs exactly as `tostring` would run it.  A failing __tostring must
    // not make dump raise: show the failure instead.
    Ok(Some(match value.to_string() {
        Ok(s) => s,
        Err(e) => format!("__tostring error: {e}"),
    }))
}

fn write_tagged(out: &mut String, s: &str) {
    out.push('<');
    out.push_str(s);
    out.push('>');
}

fn write_number(out: &mut String, n: f64) {
    if n.is_nan() {
        out.push_str("nan");
    } else if n.is_infinite() {
        out.push_str(if n > 0.0 { "inf" } else { "-inf" });
    } else {
        // `{:?}` is the shortest representation that round-trips and keeps
        // a trailing `.0` on integral floats.
        write!(out, "{n:?}").unwrap();
    }
}

fn write_string(out: &mut String, bytes: &[u8]) {
    out.push('"');
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\0'..='\x1f' | '\x7f' => write!(out, "\\{:03}", c as u32).unwrap(),
                _ => out.push(c),
            }
        }
        // Bytes that are not UTF-8 (a Lua string is bytes) as `\ddd`.
        for &b in chunk.invalid() {
            write!(out, "\\{b:03}").unwrap();
        }
    }
    out.push('"');
}

fn write_table(
    out: &mut String,
    table: &LuaTable,
    opts: &Options,
    level: usize,
    path: &mut Vec<*const std::ffi::c_void>,
) -> LuaResult<()> {
    let ptr = table.to_pointer();
    if path.contains(&ptr) {
        out.push_str("<cycle>");
        return Ok(());
    }
    if matches!(opts.depth, Some(d) if level >= d) {
        out.push_str("{...}");
        return Ok(());
    }

    // Array part, then the rest.
    let len = table.raw_len();
    let mut rest: Vec<(LuaValue, LuaValue)> = Vec::new();
    for pair in table.clone().pairs::<LuaValue, LuaValue>() {
        let (k, v) = pair?;
        let in_array = matches!(k, LuaValue::Integer(i) if i >= 1 && (i as usize) <= len);
        if !in_array {
            rest.push((k, v));
        }
    }
    if len == 0 && rest.is_empty() {
        out.push_str("{}");
        return Ok(());
    }
    if opts.sort_keys {
        rest.sort_by_key(|(k, _)| key_rank(k));
    }

    path.push(ptr);
    out.push('{');
    let total = len + rest.len();
    let mut written = 0;
    let mut item = |out: &mut String,
                    path: &mut Vec<_>,
                    key: Option<&LuaValue>,
                    v: &LuaValue|
     -> LuaResult<()> {
        open_item(out, opts, level + 1);
        if let Some(k) = key {
            write_key(out, k);
            out.push_str(" = ");
        }
        write_value(out, v, opts, level + 1, path)?;
        written += 1;
        if written < total {
            out.push(',');
        }
        Ok(())
    };
    for i in 1..=len {
        let v: LuaValue = table.raw_get(i)?;
        item(out, path, None, &v)?;
    }
    for (k, v) in &rest {
        item(out, path, Some(k), v)?;
    }
    close_table(out, opts, level);
    path.pop();
    Ok(())
}

/// Start a new item: newline + indentation, or a space on one line.
fn open_item(out: &mut String, opts: &Options, level: usize) {
    if opts.indent == 0 {
        out.push(' ');
    } else {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', opts.indent * level));
    }
}

fn close_table(out: &mut String, opts: &Options, level: usize) {
    if opts.indent == 0 {
        out.push_str(" }");
    } else {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', opts.indent * level));
        out.push('}');
    }
}

/// Sort key: strings first (byte order), then integers, floats, booleans,
/// then everything else by type name.
fn key_rank(k: &LuaValue) -> (u8, Vec<u8>, i64, u64) {
    match k {
        LuaValue::String(s) => (0, s.as_bytes().to_vec(), 0, 0),
        LuaValue::Integer(i) => (1, Vec::new(), *i, 0),
        LuaValue::Number(n) => (2, Vec::new(), 0, n.to_bits() ^ (1 << 63)),
        LuaValue::Boolean(b) => (3, Vec::new(), *b as i64, 0),
        other => (4, other.type_name().as_bytes().to_vec(), 0, 0),
    }
}

fn write_key(out: &mut String, k: &LuaValue) {
    match k {
        LuaValue::String(s) => {
            let bytes = s.as_bytes();
            if is_identifier(&bytes) {
                out.push_str(std::str::from_utf8(&bytes).unwrap());
            } else {
                out.push('[');
                write_string(out, &bytes);
                out.push(']');
            }
        }
        LuaValue::Integer(i) => write!(out, "[{i}]").unwrap(),
        LuaValue::Number(n) => {
            out.push('[');
            write_number(out, *n);
            out.push(']');
        }
        LuaValue::Boolean(b) => write!(out, "[{b}]").unwrap(),
        LuaValue::LightUserData(u) if u.0.is_null() => out.push_str("[null]"),
        other => {
            out.push('[');
            write_tagged(out, other.type_name());
            out.push(']');
        }
    }
}

fn is_identifier(bytes: &[u8]) -> bool {
    const KEYWORDS: &[&[u8]] = &[
        b"and",
        b"break",
        b"do",
        b"else",
        b"elseif",
        b"end",
        b"false",
        b"for",
        b"function",
        b"goto",
        b"if",
        b"in",
        b"local",
        b"nil",
        b"not",
        b"or",
        b"repeat",
        b"return",
        b"then",
        b"true",
        b"until",
        b"while",
    ];
    let Some(&first) = bytes.first() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_')
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
        && !KEYWORDS.contains(&bytes)
}

#[cfg(test)]
mod tests {
    use crate::util::test_eval as eval;

    #[test]
    fn scalars() {
        let s: String = eval(
            r#"
            local p = std.pretty.dump
            return table.concat({
                p(nil), p(true), p(42), p(3.0), p(0.1), p(1/0), p(-1/0), p(0/0),
                p("a\"b\\c\n\t\1"), p("café"), p(std.json.null), p(print),
            }, "|")
        "#,
        );
        assert_eq!(
            s,
            r#"nil|true|42|3.0|0.1|inf|-inf|nan|"a\"b\\c\n\t\001"|"café"|null|<function>"#
        );
    }

    #[test]
    fn non_utf8_bytes_are_escaped() {
        let s: String = eval(r#"return std.pretty.dump("a\255b")"#);
        assert_eq!(s, r#""a\255b""#);
    }

    #[test]
    fn nested_table_multiline_with_sorted_keys() {
        let s: String = eval(
            r#"
            return std.pretty.dump({ b = 2, a = { 1, 2 }, ["not id"] = true, [10] = "x", [true] = 1, [2.5] = 0 })
        "#,
        );
        let expected = "{\n  a = {\n    1,\n    2\n  },\n  b = 2,\n  [\"not id\"] = true,\n  [10] = \"x\",\n  [2.5] = 0,\n  [true] = 1\n}";
        assert_eq!(s, expected);
    }

    #[test]
    fn one_line_and_empty() {
        let s: String = eval(r#"return std.pretty.dump({ 1, { x = true }, {} }, { indent = 0 })"#);
        assert_eq!(s, "{ 1, { x = true }, {} }");
    }

    #[test]
    fn keywords_and_array_part_after_holes() {
        // `end` is a keyword, so it is bracketed; 1..#t is the array part
        // and 3 (past the border) is a plain key.
        let s: String =
            eval(r#"return std.pretty.dump({ "a", ["end"] = 1, [3] = "c" }, { indent = 0 })"#);
        assert_eq!(s, r#"{ "a", ["end"] = 1, [3] = "c" }"#);
    }

    #[test]
    fn cycle_and_depth() {
        let s: String = eval(
            r#"
            local t = { name = "t" }
            t.self = t
            t.child = { t = t, deep = { deeper = { deepest = 1 } } }
            return std.pretty.dump(t, { indent = 0 }) .. "|" .. std.pretty.dump(t, { indent = 0, depth = 2 })
        "#,
        );
        assert_eq!(
            s,
            r#"{ child = { deep = { deeper = { deepest = 1 } }, t = <cycle> }, name = "t", self = <cycle> }|{ child = { deep = {...}, t = <cycle> }, name = "t", self = <cycle> }"#
        );
    }

    #[test]
    fn tostring_metamethod_wins_and_cannot_raise() {
        let s: String = eval(
            r#"
            local ok = setmetatable({}, { __tostring = function() return "Point(1, 2)" end })
            local bad = setmetatable({}, { __tostring = function() error("boom") end })
            return std.pretty.dump({ ok, bad }, { indent = 0 })
        "#,
        );
        assert!(
            s.starts_with(r#"{ <Point(1, 2)>, <__tostring error: "#),
            "{s}"
        );
    }

    #[test]
    fn same_table_twice_is_not_a_cycle() {
        let s: String = eval(
            r#"
            local shared = { 1 }
            return std.pretty.dump({ shared, shared }, { indent = 0 })
        "#,
        );
        assert_eq!(s, "{ { 1 }, { 1 } }");
    }

    #[test]
    fn output_is_stable_across_states() {
        // Hash order differs between Lua states; the sorted dump must not.
        let code = r#"
            local t = {}
            for i = 1, 50 do t["k" .. i] = i end
            return std.pretty.dump(t, { indent = 0 })
        "#;
        let a: String = eval(code);
        let b: String = eval(code);
        assert_eq!(a, b);
        assert!(a.starts_with("{ k1 = 1, k10 = 10, k11 = 11,"), "{a}");
    }
}
