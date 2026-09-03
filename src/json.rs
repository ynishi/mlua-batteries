//! JSON encode/decode module.
//!
//! ```lua
//! local json = std.json
//! local t = json.decode('{"a":1}')
//! local s = json.encode(t)
//! local s2 = json.encode_pretty(t)
//! ```
//!
//! # Empty tables and the `__jsontype` tag
//!
//! An empty Lua table `{}` is ambiguous: it can stand for an empty JSON
//! object (`{}`) or an empty JSON array (`[]`).  Without further
//! information it is encoded as an **object**, matching the `classify`
//! heuristic: a table with `raw_len() == 0` is treated as a map.
//!
//! To ask for an empty array, tag the table's metatable with
//! `__jsontype = "array"` — the convention used by `dkjson` and
//! compatible encoders:
//!
//! ```lua
//! json.encode({})                                          --> "{}"
//! json.encode(setmetatable({}, {__jsontype = "object"}))    --> "{}"  (explicit)
//! json.encode(setmetatable({}, {__jsontype = "array"}))     --> "[]"
//! json.encode(json.array())                                 --> "[]"  (same thing)
//! ```
//!
//! An empty table carrying the metatable that mlua's serde bridge attaches
//! to sequences (a metatable whose only entry is `__metatable = false`) is
//! encoded as `[]` too, so empty sequences produced through mlua's serde
//! bridge survive a re-encode.
//!
//! `decode("[]")` returns an empty table tagged with `__jsontype = "array"`,
//! which closes the round-trip: `encode(decode("[]")) == "[]"`.  That
//! metatable is shared per Lua state and left unprotected (`getmetatable`
//! returns it); treat it as read-only — mutating it affects every tagged
//! table in the state.
//!
//! The tag is consulted **only for empty tables**.  Tables with contents are
//! classified by those contents exactly as before, so `__jsontype` can
//! neither turn a map into an array nor an array into a map.

use mlua::prelude::*;
use serde_json::Value as JsonValue;

use crate::policy::PathOp;
use crate::util::{check_path, classify, with_config, TableKind};

/// Metatable field that disambiguates an empty table (`dkjson` convention).
const JSONTYPE_KEY: &str = "__jsontype";
/// Value of [`JSONTYPE_KEY`] that requests array encoding.
const JSONTYPE_ARRAY: &str = "array";
/// Registry slot holding the per-state `{__jsontype = "array"}` metatable.
const ARRAY_MT_REGISTRY_KEY: &str = "__mlua_batteries_json_array_mt";

/// Return the shared `{__jsontype = "array"}` metatable of this Lua state,
/// creating it on first use.
///
/// Cached in the Lua registry (same pattern as the schema registry) so that
/// decoding many empty arrays does not allocate a metatable per value, and
/// so tagged tables can be compared by metatable identity from Lua.
///
/// `pub(crate)` because the NULL-preserving converters in [`crate::sql`]
/// (used by `std.sql` / `std.kv`) hand out the *same* metatable instance —
/// a table tagged by one bridge must be recognized by the other.
pub(crate) fn array_metatable(lua: &Lua) -> LuaResult<LuaTable> {
    let cached: LuaValue = lua.named_registry_value(ARRAY_MT_REGISTRY_KEY)?;
    if let LuaValue::Table(mt) = cached {
        return Ok(mt);
    }
    let mt = lua.create_table()?;
    mt.raw_set(JSONTYPE_KEY, JSONTYPE_ARRAY)?;
    lua.set_named_registry_value(ARRAY_MT_REGISTRY_KEY, mt.clone())?;
    Ok(mt)
}

/// Does this table ask to be encoded as a JSON array?
///
/// Only meaningful for empty tables — the caller checks that first.
///
/// `pub(crate)` so that the NULL-preserving encoder in [`crate::sql`]
/// applies the same rule instead of duplicating it.
pub(crate) fn wants_empty_array(table: &LuaTable) -> LuaResult<bool> {
    let Some(mt) = table.metatable() else {
        return Ok(false);
    };
    match mt.raw_get::<LuaValue>(JSONTYPE_KEY)? {
        // `"object"` (or any other value) means "not an array".
        LuaValue::String(s) => Ok(s.to_str()? == JSONTYPE_ARRAY),
        // No explicit tag: fall back to mlua's serde array metatable.
        _ => is_mlua_serde_array_metatable(&mt),
    }
}

/// Recognize the metatable that mlua's serde bridge attaches to sequences.
///
/// `LuaSerdeExt::array_metatable` only exists behind mlua's `serde` feature
/// (which this crate does not enable) and `Table::has_array_metatable` is
/// private to mlua, so the metatable is matched on its shape instead: its
/// single entry is `__metatable = false`.  Only empty tables reach this
/// check, so a shape collision (an empty table protected by a bare
/// `{__metatable = false}` metatable) can at worst turn `{}` into `[]`; it
/// can never change how a table with contents is encoded.
fn is_mlua_serde_array_metatable(mt: &LuaTable) -> LuaResult<bool> {
    let mut seen_protect_entry = false;
    for pair in mt.pairs::<LuaValue, LuaValue>() {
        let (key, value) = pair?;
        let is_protect_entry = match (&key, &value) {
            (LuaValue::String(k), LuaValue::Boolean(false)) => k.to_str()? == "__metatable",
            _ => false,
        };
        if !is_protect_entry {
            return Ok(false);
        }
        seen_protect_entry = true;
    }
    Ok(seen_protect_entry)
}

pub fn module(lua: &Lua) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;

    t.set(
        "decode",
        lua.create_function(|lua, s: String| {
            let max_depth = with_config(lua, |c| c.max_json_depth)?;
            let value: JsonValue = serde_json::from_str(&s)
                .map_err(|e| LuaError::external(format!("json.decode: {e}")))?;
            json_to_lua(lua, &value, max_depth)
        })?,
    )?;

    t.set(
        "encode",
        lua.create_function(|lua, value: LuaValue| {
            let max_depth = with_config(lua, |c| c.max_json_depth)?;
            let json = lua_to_json(&value, max_depth)?;
            serde_json::to_string(&json)
                .map_err(|e| LuaError::external(format!("json.encode: {e}")))
        })?,
    )?;

    t.set(
        "encode_pretty",
        lua.create_function(|lua, value: LuaValue| {
            let max_depth = with_config(lua, |c| c.max_json_depth)?;
            let json = lua_to_json(&value, max_depth)?;
            serde_json::to_string_pretty(&json)
                .map_err(|e| LuaError::external(format!("json.encode_pretty: {e}")))
        })?,
    )?;

    // json.array() -> empty table tagged with `__jsontype = "array"`.
    // Encodes as `[]` while empty; once elements are appended it is a plain
    // sequence and encodes as an array on its own merits.
    t.set(
        "array",
        lua.create_function(|lua, ()| {
            let arr = lua.create_table()?;
            arr.set_metatable(Some(array_metatable(lua)?))?;
            Ok(arr)
        })?,
    )?;

    t.set(
        "read_file",
        lua.create_function(|lua, path: String| {
            let access = check_path(lua, &path, PathOp::Read)?;
            let max_depth = with_config(lua, |c| c.max_json_depth)?;
            let content = access.read_to_string().map_err(LuaError::external)?;
            let value: JsonValue = serde_json::from_str(&content)
                .map_err(|e| LuaError::external(format!("json.read_file: {e}")))?;
            json_to_lua(lua, &value, max_depth)
        })?,
    )?;

    t.set(
        "write_file",
        lua.create_function(|lua, (path, value): (String, LuaValue)| {
            let access = check_path(lua, &path, PathOp::Write)?;
            let max_depth = with_config(lua, |c| c.max_json_depth)?;
            let json = lua_to_json(&value, max_depth)?;
            let content = serde_json::to_string_pretty(&json)
                .map_err(|e| LuaError::external(format!("json.write_file: {e}")))?;
            access
                .write(content.as_bytes())
                .map_err(LuaError::external)?;
            Ok(true)
        })?,
    )?;

    Ok(t)
}

// ─── Conversion: JSON → Lua ────────────────────────────

pub(crate) fn json_to_lua(lua: &Lua, value: &JsonValue, max_depth: usize) -> LuaResult<LuaValue> {
    json_to_lua_inner(lua, value, 0, max_depth)
}

fn json_to_lua_inner(
    lua: &Lua,
    value: &JsonValue,
    depth: usize,
    max_depth: usize,
) -> LuaResult<LuaValue> {
    if depth > max_depth {
        return Err(LuaError::external(format!(
            "JSON nesting too deep (limit: {max_depth})"
        )));
    }
    match value {
        JsonValue::Null => Ok(LuaValue::Nil),
        JsonValue::Bool(b) => Ok(LuaValue::Boolean(*b)),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(LuaValue::Integer(i))
            } else if let Some(f) = n.as_f64() {
                Ok(LuaValue::Number(f))
            } else {
                Err(LuaError::external(format!(
                    "JSON number {n} is not representable as i64 or f64"
                )))
            }
        }
        JsonValue::String(s) => lua.create_string(s).map(LuaValue::String),
        JsonValue::Array(arr) => {
            let table = lua.create_table()?;
            for (i, v) in arr.iter().enumerate() {
                table.set(i + 1, json_to_lua_inner(lua, v, depth + 1, max_depth)?)?;
            }
            if arr.is_empty() {
                // An empty array is indistinguishable from an empty object
                // once it is a bare Lua table — tag it so that re-encoding
                // yields `[]`.  Non-empty arrays need no tag.
                table.set_metatable(Some(array_metatable(lua)?))?;
            }
            Ok(LuaValue::Table(table))
        }
        JsonValue::Object(map) => {
            let table = lua.create_table()?;
            for (k, v) in map {
                table.set(k.as_str(), json_to_lua_inner(lua, v, depth + 1, max_depth)?)?;
            }
            Ok(LuaValue::Table(table))
        }
    }
}

// ─── Conversion: Lua → JSON ────────────────────────────

pub(crate) fn lua_to_json(value: &LuaValue, max_depth: usize) -> LuaResult<JsonValue> {
    lua_to_json_inner(value, 0, max_depth)
}

fn lua_to_json_inner(value: &LuaValue, depth: usize, max_depth: usize) -> LuaResult<JsonValue> {
    if depth > max_depth {
        return Err(LuaError::external(format!(
            "Lua table nesting too deep for JSON (limit: {max_depth})"
        )));
    }
    match value {
        LuaValue::Nil => Ok(JsonValue::Null),
        // `mlua::serde::LuaSerdeExt` maps JSON `null` to `Value::NULL`, which is
        // `LightUserData(ptr::null_mut())`.  Recognize that sentinel so values
        // produced by mlua's serde bridge can round-trip through `json.encode`.
        // Non-null `LightUserData` (app-specific) continues to error below.
        LuaValue::LightUserData(u) if u.0.is_null() => Ok(JsonValue::Null),
        LuaValue::Boolean(b) => Ok(JsonValue::Bool(*b)),
        LuaValue::Integer(i) => Ok(JsonValue::Number((*i).into())),
        LuaValue::Number(n) => serde_json::Number::from_f64(*n)
            .map(JsonValue::Number)
            .ok_or_else(|| LuaError::external(format!("cannot convert {n} to JSON number"))),
        LuaValue::String(s) => Ok(JsonValue::String(s.to_str()?.to_string())),
        LuaValue::Table(t) => lua_table_to_json(t, depth, max_depth),
        _ => Err(LuaError::external("unsupported type for JSON conversion")),
    }
}

fn lua_table_to_json(table: &LuaTable, depth: usize, max_depth: usize) -> LuaResult<JsonValue> {
    match classify(table)? {
        TableKind::Array(len) => {
            let mut arr = Vec::with_capacity(len);
            for i in 1..=len {
                let v: LuaValue = table.raw_get(i)?;
                arr.push(lua_to_json_inner(&v, depth + 1, max_depth)?);
            }
            Ok(JsonValue::Array(arr))
        }
        TableKind::Map(pairs) => {
            // `classify` reports every empty table as a map.  Consult the
            // metatable tag before committing to `{}` — tables with contents
            // never reach this branch with `pairs` empty, so their encoding
            // is untouched.
            if pairs.is_empty() && wants_empty_array(table)? {
                return Ok(JsonValue::Array(Vec::new()));
            }
            let mut map = serde_json::Map::new();
            for (k, v) in pairs {
                let key = match k {
                    LuaValue::String(s) => s.to_str()?.to_string(),
                    LuaValue::Integer(i) => i.to_string(),
                    LuaValue::Number(n) => n.to_string(),
                    other => {
                        return Err(LuaError::external(format!(
                            "unsupported table key type for JSON: {}",
                            other.type_name()
                        )));
                    }
                };
                map.insert(key, lua_to_json_inner(&v, depth + 1, max_depth)?);
            }
            Ok(JsonValue::Object(map))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::util::test_eval as eval;

    #[test]
    fn decode_object() {
        let s: String = eval(
            r#"
            local t = std.json.decode('{"a":1,"b":"hello"}')
            return tostring(t.a) .. "," .. t.b
        "#,
        );
        assert_eq!(s, "1,hello");
    }

    #[test]
    fn decode_array() {
        let n: i64 = eval(
            r#"
            local arr = std.json.decode('[10,20,30]')
            return #arr
        "#,
        );
        assert_eq!(n, 3);
    }

    #[test]
    fn decode_invalid_returns_error() {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let result: mlua::Result<LuaValue> = lua.load(r#"return std.json.decode("{bad")"#).eval();
        assert!(result.is_err());
    }

    #[test]
    fn encode_roundtrip() {
        let s: String = eval(
            r#"
            local original = '{"name":"test","values":[1,2,3]}'
            local t = std.json.decode(original)
            local encoded = std.json.encode(t)
            local t2 = std.json.decode(encoded)
            return t2.name .. "," .. tostring(#t2.values)
        "#,
        );
        assert_eq!(s, "test,3");
    }

    #[test]
    fn encode_empty_table_as_object() {
        let s: String = eval(
            r#"
            return std.json.encode({})
        "#,
        );
        assert_eq!(s, "{}");
    }

    // ─── Empty-table `__jsontype` tag ──────────────────

    #[test]
    fn encode_tagged_empty_table_as_array() {
        let s: String = eval(
            r#"
            return std.json.encode(setmetatable({}, {__jsontype = "array"}))
        "#,
        );
        assert_eq!(s, "[]");
    }

    #[test]
    fn encode_pretty_tagged_empty_table_as_array() {
        let s: String = eval(
            r#"
            return std.json.encode_pretty(setmetatable({}, {__jsontype = "array"}))
        "#,
        );
        assert_eq!(s, "[]");
    }

    #[test]
    fn encode_object_tagged_empty_table_as_object() {
        // The `"object"` tag is recognized for symmetry; it matches the
        // default, so it is a no-op that documents intent.
        let s: String = eval(
            r#"
            return std.json.encode(setmetatable({}, {__jsontype = "object"}))
        "#,
        );
        assert_eq!(s, "{}");
    }

    #[test]
    fn encode_unknown_tag_on_empty_table_stays_object() {
        let s: String = eval(
            r#"
            return std.json.encode(setmetatable({}, {__jsontype = "whatever"}))
        "#,
        );
        assert_eq!(s, "{}");
    }

    #[test]
    fn encode_empty_table_with_mlua_serde_array_metatable_shape_as_array() {
        // mlua's serde bridge attaches a metatable whose only entry is
        // `__metatable = false` to every sequence it builds.  This crate does
        // not enable mlua's `serde` feature, so the test constructs a
        // metatable of the same shape — which is exactly what the encoder
        // matches on.
        let s: String = eval(
            r#"
            return std.json.encode(setmetatable({}, {__metatable = false}))
        "#,
        );
        assert_eq!(s, "[]");
    }

    #[test]
    fn encode_protected_metatable_with_extra_fields_stays_object() {
        // Guardrail on the shape match above: anything richer than the bare
        // `{__metatable = false}` metatable is not mlua's array metatable.
        let s: String = eval(
            r#"
            return std.json.encode(setmetatable({}, {__metatable = false, __index = {}}))
        "#,
        );
        assert_eq!(s, "{}");
    }

    #[test]
    fn encode_empty_table_with_unrelated_metatable_stays_object() {
        let s: String = eval(
            r#"
            return std.json.encode(setmetatable({}, {__index = {a = 1}}))
        "#,
        );
        assert_eq!(s, "{}");
    }

    #[test]
    fn encode_nonempty_map_ignores_array_tag() {
        let s: String = eval(
            r#"
            return std.json.encode(setmetatable({a = 1}, {__jsontype = "array"}))
        "#,
        );
        assert_eq!(s, r#"{"a":1}"#);
    }

    #[test]
    fn encode_nonempty_sequence_ignores_object_tag() {
        let s: String = eval(
            r#"
            return std.json.encode(setmetatable({1, 2}, {__jsontype = "object"}))
        "#,
        );
        assert_eq!(s, "[1,2]");
    }

    #[test]
    fn encode_nested_tagged_empty_array() {
        let s: String = eval(
            r#"
            return std.json.encode({items = setmetatable({}, {__jsontype = "array"}), n = 0})
        "#,
        );
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert!(v["items"].is_array());
        assert_eq!(v["items"].as_array().unwrap().len(), 0);
        assert_eq!(v["n"], 0);
    }

    // ─── decode tags empty arrays ──────────────────────

    #[test]
    fn decode_empty_array_is_tagged() {
        let s: String = eval(
            r#"
            local mt = getmetatable(std.json.decode('[]'))
            return tostring(mt ~= nil) .. "," .. tostring(mt.__jsontype)
        "#,
        );
        assert_eq!(s, "true,array");
    }

    #[test]
    fn decode_empty_array_roundtrips() {
        let s: String = eval(
            r#"
            return std.json.encode(std.json.decode('[]'))
        "#,
        );
        assert_eq!(s, "[]");
    }

    #[test]
    fn decode_empty_object_roundtrips() {
        let s: String = eval(
            r#"
            return std.json.encode(std.json.decode('{}'))
        "#,
        );
        assert_eq!(s, "{}");
    }

    #[test]
    fn decode_nested_empty_array_roundtrips() {
        let s: String = eval(
            r#"
            return std.json.encode(std.json.decode('{"a":[],"b":{},"c":[[]]}'))
        "#,
        );
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert!(v["a"].is_array());
        assert!(v["b"].is_object());
        assert!(v["c"][0].is_array());
    }

    #[test]
    fn decode_nonempty_array_is_untagged() {
        let b: bool = eval(
            r#"
            return getmetatable(std.json.decode('[1,2,3]')) == nil
        "#,
        );
        assert!(b);
    }

    #[test]
    fn decode_empty_array_stays_usable_as_a_table() {
        // The tag metatable carries no `__index` / `__newindex`, so a decoded
        // empty array behaves like the plain table it used to be.
        let s: String = eval(
            r#"
            local a = std.json.decode('[]')
            a[1] = "x"
            return tostring(#a) .. "," .. tostring(a.missing) .. "," .. std.json.encode(a)
        "#,
        );
        assert_eq!(s, r#"1,nil,["x"]"#);
    }

    #[test]
    fn decoded_empty_arrays_share_one_metatable() {
        // The metatable is cached in the Lua registry, so decoding N empty
        // arrays does not allocate N metatables.
        let b: bool = eval(
            r#"
            local a = std.json.decode('[]')
            local b = std.json.decode('{"x":[]}').x
            return getmetatable(a) == getmetatable(b)
        "#,
        );
        assert!(b);
    }

    // ─── json.array() helper ───────────────────────────

    #[test]
    fn array_helper_encodes_as_array() {
        let s: String = eval(
            r#"
            return std.json.encode(std.json.array())
        "#,
        );
        assert_eq!(s, "[]");
    }

    #[test]
    fn array_helper_accepts_elements() {
        let s: String = eval(
            r#"
            local a = std.json.array()
            a[1] = "x"
            a[2] = "y"
            return std.json.encode(a)
        "#,
        );
        assert_eq!(s, r#"["x","y"]"#);
    }

    #[test]
    fn array_helper_shares_the_decode_metatable() {
        let b: bool = eval(
            r#"
            return getmetatable(std.json.array()) == getmetatable(std.json.decode('[]'))
        "#,
        );
        assert!(b);
    }

    #[test]
    fn write_file_and_read_file_preserve_empty_array() {
        let dir = std::env::temp_dir().join("mlua_bat_test_json_empty_array");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("data.json");
        let path_str = path.to_string_lossy();

        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();

        let s: String = lua
            .load(format!(
                r#"
                std.json.write_file("{path_str}", {{items = std.json.array()}})
                local t = std.json.read_file("{path_str}")
                return std.json.encode(t)
            "#
            ))
            .eval()
            .unwrap();
        assert_eq!(s, r#"{"items":[]}"#);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn encode_nested_structure() {
        let s: String = eval(
            r#"
            return std.json.encode({items = {1, 2}, meta = {ok = true}})
        "#,
        );
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert!(v["items"].is_array());
        assert_eq!(v["meta"]["ok"], true);
    }

    #[test]
    fn encode_pretty_has_newlines() {
        let s: String = eval(
            r#"
            return std.json.encode_pretty({a = 1})
        "#,
        );
        assert!(s.contains('\n'));
    }

    #[test]
    fn decode_null_becomes_nil() {
        let b: bool = eval(
            r#"
            local t = std.json.decode('{"x":null}')
            return t.x == nil
        "#,
        );
        assert!(b);
    }

    #[test]
    fn decode_boolean() {
        let s: String = eval(
            r#"
            local t = std.json.decode('{"flag":true}')
            return type(t.flag)
        "#,
        );
        assert_eq!(s, "boolean");
    }

    #[test]
    fn max_depth_enforced_on_decode() {
        let lua = Lua::new();
        let config = crate::config::Config::builder()
            .max_json_depth(2)
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        // depth 3: {"a":{"b":{"c":1}}}
        let result: mlua::Result<LuaValue> = lua
            .load(r#"return std.json.decode('{"a":{"b":{"c":1}}}')"#)
            .eval();
        assert!(result.is_err());
    }

    #[test]
    fn max_depth_enforced_on_encode() {
        let lua = Lua::new();
        let config = crate::config::Config::builder()
            .max_json_depth(2)
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        // depth 3 nested table
        let result: mlua::Result<LuaValue> = lua
            .load(r#"return std.json.encode({a = {b = {c = 1}}})"#)
            .eval();
        assert!(result.is_err());
    }

    #[test]
    fn encode_rejects_boolean_key() {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();

        let result: mlua::Result<LuaValue> = lua
            .load(
                r#"
                local t = {}
                t[true] = "val"
                return std.json.encode(t)
            "#,
            )
            .eval();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("unsupported table key type"));
    }

    #[test]
    fn encode_accepts_mlua_null_sentinel() {
        // `mlua::Value::NULL` is the sentinel produced by `LuaSerdeExt::to_value`
        // for JSON `null`.  Encode must map it back to JSON `null` so that values
        // going through mlua's serde bridge can be re-encoded.
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();

        lua.globals().set("_null", LuaValue::NULL).unwrap();

        let s: String = lua
            .load(r#"return std.json.encode({ x = _null, y = 1 })"#)
            .eval()
            .unwrap();

        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert!(v["x"].is_null());
        assert_eq!(v["y"], 1);
    }

    #[test]
    fn encode_rejects_non_null_light_userdata() {
        // Guardrail: only the canonical NULL sentinel is accepted.  App-specific
        // LightUserData pointers must still error — we don't want to silently
        // serialize arbitrary pointers as `null`.
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();

        let mut dummy = 42u8;
        let ud = LuaValue::LightUserData(mlua::LightUserData(
            &mut dummy as *mut _ as *mut std::ffi::c_void,
        ));
        lua.globals().set("_ud", ud).unwrap();

        let result: mlua::Result<String> = lua.load(r#"return std.json.encode(_ud)"#).eval();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("unsupported type for JSON conversion"));
    }

    #[test]
    fn read_file_and_write_file_roundtrip() {
        let dir = std::env::temp_dir().join("mlua_bat_test_json_file");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("data.json");
        let path_str = path.to_string_lossy();

        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();

        let s: String = lua
            .load(format!(
                r#"
                std.json.write_file("{path_str}", {{name = "test", ok = true}})
                local t = std.json.read_file("{path_str}")
                return t.name
            "#
            ))
            .eval()
            .unwrap();
        assert_eq!(s, "test");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
