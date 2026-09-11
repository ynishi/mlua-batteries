//! `preload_all`: modules reachable through `require`, no global installed.

use mlua::prelude::*;

#[test]
fn require_reaches_each_module_and_no_global_is_set() {
    let lua = Lua::new();
    mlua_batteries::preload_all(&lua, mlua_batteries::PRELOAD_PREFIX).unwrap();

    // Every enabled module is loadable under the prefix.
    for (name, _) in mlua_batteries::module_entries() {
        let ok: bool = lua
            .load(format!(
                r#"return type(require("mlua_batteries.{name}")) == "table""#
            ))
            .eval()
            .unwrap();
        assert!(
            ok,
            "require(\"mlua_batteries.{name}\") did not return a table"
        );
    }

    // No global leaks: neither the prefix nor `std`.
    let globals: bool = lua
        .load(r#"return rawget(_G, "mlua_batteries") == nil and rawget(_G, "std") == nil"#)
        .eval()
        .unwrap();
    assert!(globals);
}

#[test]
fn namespace_shares_instances_with_per_module_require() {
    let lua = Lua::new();
    mlua_batteries::preload_all(&lua, "bat").unwrap();

    let same: bool = lua
        .load(
            r#"
            local ns = require("bat")
            return ns.json == require("bat.json") and ns.json.encode({a = 1}) == '{"a":1}'
        "#,
        )
        .eval()
        .unwrap();
    assert!(same);
}

#[test]
fn preloaded_module_uses_the_config() {
    use mlua_batteries::config::Config;

    let lua = Lua::new();
    let config = Config::builder().max_json_depth(1).build().unwrap();
    mlua_batteries::preload_all_with(&lua, "bat", config).unwrap();

    let result: LuaResult<LuaValue> = lua
        .load(r#"return require("bat.json").decode('[[[1]]]')"#)
        .eval();
    assert!(
        result.is_err(),
        "max_json_depth from the Config should apply"
    );
}
