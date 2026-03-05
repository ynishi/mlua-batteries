//! Table validation module.
//!
//! Validates Lua table structure against a schema definition.
//! Schemas are plain Lua tables — no external schema language required.
//!
//! # Schema formats
//!
//! **Shorthand** — type name only (field is optional):
//!
//! ```lua
//! local schema = {name = "string", age = "number"}
//! ```
//!
//! **Full** — table with constraints:
//!
//! ```lua
//! local schema = {
//!     name   = {type = "string", required = true, min_len = 1},
//!     age    = {type = "number", min = 0, max = 150},
//!     status = {type = "string", one_of = {"active", "inactive"}},
//!     tags   = {type = "table"},
//! }
//! ```
//!
//! # Supported constraints
//!
//! | Key | Applies to | Description |
//! |-----|-----------|-------------|
//! | `type` | all | Expected Lua type name |
//! | `required` | all | Field must be non-nil (default: false) |
//! | `min` | number/integer | Minimum value (inclusive) |
//! | `max` | number/integer | Maximum value (inclusive) |
//! | `min_len` | string | Minimum string length |
//! | `max_len` | string | Maximum string length |
//! | `one_of` | string/number/integer/boolean | Allowed values list |
//!
//! # Usage
//!
//! ```lua
//! local ok, errors = std.validate.check(data, schema)
//! if not ok then
//!     for _, msg in ipairs(errors) do print(msg) end
//! end
//! ```

use mlua::prelude::*;

pub fn module(lua: &Lua) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;

    t.set(
        "check",
        lua.create_function(|lua, (data, schema): (LuaTable, LuaTable)| {
            let mut errors: Vec<String> = Vec::new();
            validate_table(&data, &schema, &mut errors)?;
            if errors.is_empty() {
                Ok((true, LuaValue::Nil))
            } else {
                let err_table = lua.create_table()?;
                for (i, e) in errors.iter().enumerate() {
                    err_table.set(i + 1, e.as_str())?;
                }
                Ok((false, LuaValue::Table(err_table)))
            }
        })?,
    )?;

    Ok(t)
}

struct FieldSpec {
    type_name: Option<String>,
    required: bool,
    min: Option<f64>,
    max: Option<f64>,
    min_len: Option<usize>,
    max_len: Option<usize>,
    one_of: Option<Vec<LuaValue>>,
}

fn parse_field_spec(value: &LuaValue) -> LuaResult<FieldSpec> {
    match value {
        LuaValue::String(s) => Ok(FieldSpec {
            type_name: Some(s.to_str()?.to_string()),
            required: false,
            min: None,
            max: None,
            min_len: None,
            max_len: None,
            one_of: None,
        }),
        LuaValue::Table(t) => {
            let type_name: Option<String> = t.get("type")?;
            let required: Option<bool> = t.get("required")?;
            let min: Option<f64> = t.get("min")?;
            let max: Option<f64> = t.get("max")?;
            let min_len: Option<usize> = t.get("min_len")?;
            let max_len: Option<usize> = t.get("max_len")?;
            let one_of_table: Option<LuaTable> = t.get("one_of")?;
            let one_of = match one_of_table {
                Some(tbl) => {
                    let mut vals = Vec::new();
                    for v in tbl.sequence_values::<LuaValue>() {
                        vals.push(v?);
                    }
                    Some(vals)
                }
                None => None,
            };
            Ok(FieldSpec {
                type_name,
                required: required.unwrap_or(false),
                min,
                max,
                min_len,
                max_len,
                one_of,
            })
        }
        other => Err(LuaError::external(format!(
            "validate: schema field must be a string or table, got {}",
            other.type_name()
        ))),
    }
}

fn validate_table(data: &LuaTable, schema: &LuaTable, errors: &mut Vec<String>) -> LuaResult<()> {
    for pair in schema.pairs::<LuaValue, LuaValue>() {
        let (key, spec_value) = pair?;
        let key_str = format_key(&key);
        let spec = parse_field_spec(&spec_value)?;
        let value: LuaValue = data.get(key)?;
        validate_field(&key_str, &value, &spec, errors);
    }
    Ok(())
}

fn validate_field(key: &str, value: &LuaValue, spec: &FieldSpec, errors: &mut Vec<String>) {
    // nil check
    if matches!(value, LuaValue::Nil) {
        if spec.required {
            errors.push(format!("{key}: required"));
        }
        return;
    }

    // type check
    if let Some(ref expected) = spec.type_name {
        if !matches_type(value, expected) {
            errors.push(format!(
                "{key}: expected {expected}, got {}",
                lua_type_name(value)
            ));
            return; // skip further checks if type is wrong
        }
    }

    // numeric range
    if let Some(n) = as_number(value) {
        if let Some(min) = spec.min {
            if n < min {
                errors.push(format!("{key}: must be >= {min}, got {n}"));
            }
        }
        if let Some(max) = spec.max {
            if n > max {
                errors.push(format!("{key}: must be <= {max}, got {n}"));
            }
        }
    }

    // string length
    if let LuaValue::String(s) = value {
        let len = s.as_bytes().len();
        if let Some(min_len) = spec.min_len {
            if len < min_len {
                errors.push(format!("{key}: length must be >= {min_len}, got {len}"));
            }
        }
        if let Some(max_len) = spec.max_len {
            if len > max_len {
                errors.push(format!("{key}: length must be <= {max_len}, got {len}"));
            }
        }
    }

    // one_of
    if let Some(ref allowed) = spec.one_of {
        if !allowed.iter().any(|a| values_equal(a, value)) {
            let allowed_str = allowed
                .iter()
                .map(format_display)
                .collect::<Vec<_>>()
                .join(", ");
            errors.push(format!(
                "{key}: must be one of [{allowed_str}], got {}",
                format_display(value)
            ));
        }
    }
}

fn matches_type(value: &LuaValue, expected: &str) -> bool {
    match expected {
        "string" => matches!(value, LuaValue::String(_)),
        "number" => matches!(value, LuaValue::Number(_) | LuaValue::Integer(_)),
        "integer" => matches!(value, LuaValue::Integer(_)),
        "boolean" => matches!(value, LuaValue::Boolean(_)),
        "table" => matches!(value, LuaValue::Table(_)),
        "function" => matches!(value, LuaValue::Function(_)),
        "any" => true,
        _ => false,
    }
}

fn lua_type_name(value: &LuaValue) -> &'static str {
    match value {
        LuaValue::Nil => "nil",
        LuaValue::Boolean(_) => "boolean",
        LuaValue::Integer(_) => "integer",
        LuaValue::Number(_) => "number",
        LuaValue::String(_) => "string",
        LuaValue::Table(_) => "table",
        LuaValue::Function(_) => "function",
        _ => "userdata",
    }
}

fn as_number(value: &LuaValue) -> Option<f64> {
    match value {
        LuaValue::Number(n) => Some(*n),
        LuaValue::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

fn values_equal(a: &LuaValue, b: &LuaValue) -> bool {
    match (a, b) {
        (LuaValue::String(a), LuaValue::String(b)) => a.as_bytes() == b.as_bytes(),
        (LuaValue::Integer(a), LuaValue::Integer(b)) => a == b,
        (LuaValue::Number(a), LuaValue::Number(b)) => a == b,
        (LuaValue::Integer(a), LuaValue::Number(b)) => (*a as f64) == *b,
        (LuaValue::Number(a), LuaValue::Integer(b)) => *a == (*b as f64),
        (LuaValue::Boolean(a), LuaValue::Boolean(b)) => a == b,
        (LuaValue::Nil, LuaValue::Nil) => true,
        _ => false,
    }
}

fn format_key(value: &LuaValue) -> String {
    match value {
        LuaValue::String(s) => s.to_string_lossy().to_string(),
        LuaValue::Integer(i) => i.to_string(),
        other => format!("<{}>", other.type_name()),
    }
}

fn format_display(value: &LuaValue) -> String {
    match value {
        LuaValue::Nil => "nil".to_string(),
        LuaValue::Boolean(b) => b.to_string(),
        LuaValue::Integer(i) => i.to_string(),
        LuaValue::Number(n) => n.to_string(),
        LuaValue::String(s) => format!("\"{}\"", s.to_string_lossy()),
        other => format!("<{}>", other.type_name()),
    }
}

#[cfg(test)]
mod tests {
    use crate::util::test_eval as eval;

    // ─── shorthand (type-only) ────────────────────────────

    #[test]
    fn shorthand_valid() {
        let ok: bool = eval(
            r#"
            local ok, _ = std.validate.check(
                {name = "John", age = 30},
                {name = "string", age = "number"}
            )
            return ok
        "#,
        );
        assert!(ok);
    }

    #[test]
    fn shorthand_type_mismatch() {
        let s: String = eval(
            r#"
            local ok, errs = std.validate.check(
                {name = 42},
                {name = "string"}
            )
            return errs[1]
        "#,
        );
        assert!(s.contains("expected string, got integer"), "got: {s}");
    }

    #[test]
    fn shorthand_missing_optional_is_ok() {
        let ok: bool = eval(
            r#"
            local ok, _ = std.validate.check(
                {},
                {name = "string"}
            )
            return ok
        "#,
        );
        assert!(ok);
    }

    // ─── required ─────────────────────────────────────────

    #[test]
    fn required_missing_field() {
        let s: String = eval(
            r#"
            local ok, errs = std.validate.check(
                {},
                {name = {type = "string", required = true}}
            )
            return errs[1]
        "#,
        );
        assert!(s.contains("required"), "got: {s}");
    }

    #[test]
    fn required_present_field() {
        let ok: bool = eval(
            r#"
            local ok, _ = std.validate.check(
                {name = "John"},
                {name = {type = "string", required = true}}
            )
            return ok
        "#,
        );
        assert!(ok);
    }

    // ─── numeric range ────────────────────────────────────

    #[test]
    fn min_violated() {
        let s: String = eval(
            r#"
            local ok, errs = std.validate.check(
                {age = -1},
                {age = {type = "number", min = 0}}
            )
            return errs[1]
        "#,
        );
        assert!(s.contains(">= 0"), "got: {s}");
    }

    #[test]
    fn max_violated() {
        let s: String = eval(
            r#"
            local ok, errs = std.validate.check(
                {age = 200},
                {age = {type = "number", max = 150}}
            )
            return errs[1]
        "#,
        );
        assert!(s.contains("<= 150"), "got: {s}");
    }

    #[test]
    fn range_valid() {
        let ok: bool = eval(
            r#"
            local ok, _ = std.validate.check(
                {age = 30},
                {age = {type = "number", min = 0, max = 150}}
            )
            return ok
        "#,
        );
        assert!(ok);
    }

    // ─── string length ────────────────────────────────────

    #[test]
    fn min_len_violated() {
        let s: String = eval(
            r#"
            local ok, errs = std.validate.check(
                {name = ""},
                {name = {type = "string", min_len = 1}}
            )
            return errs[1]
        "#,
        );
        assert!(s.contains("length must be >= 1"), "got: {s}");
    }

    #[test]
    fn max_len_violated() {
        let s: String = eval(
            r#"
            local ok, errs = std.validate.check(
                {code = "ABCDEF"},
                {code = {type = "string", max_len = 3}}
            )
            return errs[1]
        "#,
        );
        assert!(s.contains("length must be <= 3"), "got: {s}");
    }

    // ─── one_of ───────────────────────────────────────────

    #[test]
    fn one_of_valid() {
        let ok: bool = eval(
            r#"
            local ok, _ = std.validate.check(
                {status = "active"},
                {status = {type = "string", one_of = {"active", "inactive"}}}
            )
            return ok
        "#,
        );
        assert!(ok);
    }

    #[test]
    fn one_of_violated() {
        let s: String = eval(
            r#"
            local ok, errs = std.validate.check(
                {status = "unknown"},
                {status = {type = "string", one_of = {"active", "inactive"}}}
            )
            return errs[1]
        "#,
        );
        assert!(s.contains("must be one of"), "got: {s}");
        assert!(s.contains("\"active\""), "got: {s}");
    }

    #[test]
    fn one_of_numeric() {
        let ok: bool = eval(
            r#"
            local ok, _ = std.validate.check(
                {level = 2},
                {level = {type = "number", one_of = {1, 2, 3}}}
            )
            return ok
        "#,
        );
        assert!(ok);
    }

    // ─── integer type ─────────────────────────────────────

    #[test]
    fn integer_accepts_integer() {
        let ok: bool = eval(
            r#"
            local ok, _ = std.validate.check(
                {count = 42},
                {count = "integer"}
            )
            return ok
        "#,
        );
        assert!(ok);
    }

    #[test]
    fn integer_rejects_float() {
        let s: String = eval(
            r#"
            local ok, errs = std.validate.check(
                {count = 3.14},
                {count = "integer"}
            )
            return errs[1]
        "#,
        );
        assert!(s.contains("expected integer, got number"), "got: {s}");
    }

    // ─── any type ─────────────────────────────────────────

    #[test]
    fn any_accepts_anything() {
        let ok: bool = eval(
            r#"
            local ok, _ = std.validate.check(
                {data = "text", count = 42, flag = true},
                {data = "any", count = "any", flag = "any"}
            )
            return ok
        "#,
        );
        assert!(ok);
    }

    // ─── multiple errors ──────────────────────────────────

    #[test]
    fn multiple_errors_collected() {
        let n: i64 = eval(
            r#"
            local ok, errs = std.validate.check(
                {name = 42, age = "old"},
                {name = "string", age = "number"}
            )
            return #errs
        "#,
        );
        assert_eq!(n, 2);
    }

    // ─── table type ───────────────────────────────────────

    #[test]
    fn table_type_valid() {
        let ok: bool = eval(
            r#"
            local ok, _ = std.validate.check(
                {tags = {"a", "b"}},
                {tags = "table"}
            )
            return ok
        "#,
        );
        assert!(ok);
    }

    #[test]
    fn table_type_rejects_string() {
        let s: String = eval(
            r#"
            local ok, errs = std.validate.check(
                {tags = "not a table"},
                {tags = "table"}
            )
            return errs[1]
        "#,
        );
        assert!(s.contains("expected table, got string"), "got: {s}");
    }

    // ─── boolean type ─────────────────────────────────────

    #[test]
    fn boolean_valid() {
        let ok: bool = eval(
            r#"
            local ok, _ = std.validate.check(
                {active = true},
                {active = "boolean"}
            )
            return ok
        "#,
        );
        assert!(ok);
    }

    // ─── edge cases ───────────────────────────────────────

    #[test]
    fn empty_schema_always_passes() {
        let ok: bool = eval(
            r#"
            local ok, _ = std.validate.check({anything = "here"}, {})
            return ok
        "#,
        );
        assert!(ok);
    }

    #[test]
    fn schema_with_invalid_spec_returns_error() {
        let lua = mlua::Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let result: mlua::Result<mlua::Value> = lua
            .load(r#"return std.validate.check({x = 1}, {x = 42})"#)
            .eval();
        assert!(result.is_err());
    }

    #[test]
    fn type_mismatch_skips_range_checks() {
        let n: i64 = eval(
            r#"
            local ok, errs = std.validate.check(
                {age = "not a number"},
                {age = {type = "number", min = 0, max = 150}}
            )
            return #errs
        "#,
        );
        // Only the type error, not min/max errors
        assert_eq!(n, 1);
    }
}
