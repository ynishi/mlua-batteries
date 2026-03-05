//! Schema bridge integration module.
//!
//! Bridges `schema_bridge::Schema` (derived from Rust structs) to
//! Lua tables compatible with [`crate::validate::check()`].
//!
//! # Usage from Rust
//!
//! ```rust,ignore
//! use schema_bridge::SchemaBridge;
//!
//! #[derive(SchemaBridge)]
//! struct User {
//!     name: String,
//!     #[schema(min = 0, max = 150)]
//!     age: i32,
//!     email: Option<String>,
//! }
//!
//! let lua = mlua::Lua::new();
//! mlua_batteries::register_all(&lua, "std").unwrap();
//!
//! // Register the schema as a Lua global
//! mlua_batteries::schema::register::<User>(&lua, "std", "User").unwrap();
//!
//! // Now Lua code can validate data against the schema:
//! // local ok, errs = std.schema.check("User", data)
//! ```

use mlua::prelude::*;

const REGISTRY_NAME: &str = "__mlua_batteries_schema_registry";

fn get_or_create_registry(lua: &Lua) -> LuaResult<LuaTable> {
    let val: LuaValue = lua.named_registry_value(REGISTRY_NAME)?;
    if let LuaValue::Table(t) = val {
        Ok(t)
    } else {
        let t = lua.create_table()?;
        lua.set_named_registry_value(REGISTRY_NAME, t.clone())?;
        Ok(t)
    }
}

/// Create the schema module table.
///
/// Provides:
/// - `schema.check(name, data)` — validate data against a named schema
/// - `schema.get(name)` — retrieve a registered schema table
/// - `schema.list()` — list all registered schema names
pub fn module(lua: &Lua) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;

    // Ensure registry exists
    get_or_create_registry(lua)?;

    // schema.get(name) -> schema_table | nil
    t.set(
        "get",
        lua.create_function(|lua, name: String| {
            let registry = get_or_create_registry(lua)?;
            let value: LuaValue = registry.get(name.as_str())?;
            Ok(value)
        })?,
    )?;

    // schema.list() -> string[]
    t.set(
        "list",
        lua.create_function(|lua, ()| {
            let registry = get_or_create_registry(lua)?;
            let result = lua.create_table()?;
            let mut i = 1;
            for pair in registry.pairs::<String, LuaValue>() {
                let (k, _) = pair?;
                result.set(i, k)?;
                i += 1;
            }
            Ok(result)
        })?,
    )?;

    // schema.check(name, data) -> bool, nil|errors
    t.set(
        "check",
        lua.create_function(|lua, (name, data): (String, LuaTable)| {
            let registry = get_or_create_registry(lua)?;
            let schema_table: LuaValue = registry.get(name.as_str())?;
            let schema_table = match schema_table {
                LuaValue::Table(t) => t,
                _ => {
                    return Err(LuaError::external(format!(
                        "schema.check: unknown schema \"{name}\""
                    )));
                }
            };
            let validate_mod = crate::validate::module(lua)?;
            let check_fn: LuaFunction = validate_mod.get("check")?;
            check_fn.call::<LuaMultiValue>((data, schema_table))
        })?,
    )?;

    Ok(t)
}

/// Register a Rust-derived schema under the given name.
///
/// The schema becomes accessible from Lua as `{namespace}.schema.get("{name}")`
/// and can be validated with `{namespace}.schema.check("{name}", data)`.
pub fn register<T: schema_bridge::SchemaBridge>(
    lua: &Lua,
    namespace: &str,
    name: &str,
) -> LuaResult<()> {
    let schema = T::to_schema();
    let lua_table = schema.to_lua_table(lua)?;

    let registry = get_or_create_registry(lua)?;
    registry.set(name, lua_table)?;

    // Ensure the schema module exists on the namespace
    let ns: LuaTable = lua.globals().get(namespace)?;
    if ns.get::<LuaValue>("schema")?.is_nil() {
        let schema_mod = module(lua)?;
        ns.set("schema", schema_mod)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use mlua::prelude::*;

    #[derive(schema_bridge::SchemaBridge)]
    struct TestUser {
        name: String,
        age: i32,
        email: Option<String>,
    }

    fn setup_lua() -> Lua {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();
        super::register::<TestUser>(&lua, "std", "TestUser").unwrap();
        lua
    }

    #[test]
    fn register_and_get() {
        let lua = setup_lua();
        let result: LuaValue = lua
            .load(r#"return std.schema.get("TestUser")"#)
            .eval()
            .unwrap();
        assert!(result.is_table());
    }

    #[test]
    fn get_unknown_returns_nil() {
        let lua = setup_lua();
        let result: LuaValue = lua
            .load(r#"return std.schema.get("Unknown")"#)
            .eval()
            .unwrap();
        assert!(result.is_nil());
    }

    #[test]
    fn list_registered_schemas() {
        let lua = setup_lua();
        let result: LuaTable = lua.load(r#"return std.schema.list()"#).eval().unwrap();
        let first: String = result.get(1).unwrap();
        assert_eq!(first, "TestUser");
    }

    #[test]
    fn check_valid_data() {
        let lua = setup_lua();
        let (ok, errs): (bool, LuaValue) = lua
            .load(
                r#"return std.schema.check("TestUser", {name = "Alice", age = 30, email = "a@b.com"})"#,
            )
            .eval()
            .unwrap();
        assert!(ok, "valid data should pass, errors: {errs:?}");
    }

    #[test]
    fn check_invalid_data() {
        let lua = setup_lua();
        let (ok, errs): (bool, LuaValue) = lua
            .load(r#"return std.schema.check("TestUser", {name = 123, age = "not a number"})"#)
            .eval()
            .unwrap();
        assert!(!ok);
        assert!(errs.is_table());
    }

    #[test]
    fn check_unknown_schema_returns_error() {
        let lua = setup_lua();
        let result: mlua::Result<LuaValue> = lua
            .load(r#"return std.schema.check("Nope", {x = 1})"#)
            .eval();
        assert!(result.is_err());
    }
}
