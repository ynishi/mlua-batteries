//! The shipped `.d.tl` files are hand-written; this is what keeps them in
//! step with the Rust modules.
//!
//! For every declaration in `dts::entries()` the matching module table is
//! built and its keys compared with the record's top-level fields, both
//! ways.  A second test runs `htl check` over the written declarations when
//! the `htl` binary is on PATH, so a declaration that is not valid Teal is
//! caught here rather than in a downstream project.

use std::collections::BTreeSet;
use std::process::Command;

use mlua::prelude::*;
use mlua_batteries::dts;

/// Top-level record fields of a `.d.tl`: lines indented by exactly three
/// spaces of the form `name: <type>`.  Nested records and `type X = …`
/// aliases are indented deeper or lack the `name:` shape, so they do not
/// count — the convention every shipped file follows.
fn declared_fields(source: &str) -> BTreeSet<String> {
    source
        .lines()
        .filter_map(|line| {
            let body = line.strip_prefix("   ")?;
            if body.starts_with(' ') {
                return None;
            }
            let (name, _) = body.split_once(':')?;
            let name = name.trim();
            let is_ident = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            is_ident.then(|| name.to_string())
        })
        .collect()
}

fn module_keys(lua: &Lua, table: &LuaTable) -> BTreeSet<String> {
    let _ = lua;
    table
        .clone()
        .pairs::<String, LuaValue>()
        .map(|pair| pair.unwrap().0)
        .collect()
}

fn build_module(lua: &Lua, name: &str) -> LuaTable {
    if let Some((_, factory)) = mlua_batteries::module_entries()
        .into_iter()
        .find(|(n, _)| *n == name)
    {
        return factory(lua).unwrap();
    }
    #[cfg(feature = "task")]
    if name == "task" {
        return mlua_batteries::task::module(lua).unwrap();
    }
    panic!("no module factory for declaration `{name}`");
}

#[test]
fn declared_fields_match_module_keys() {
    let lua = Lua::new();
    // Modules read their Config from app_data when called, not when built,
    // but keep the state well-formed anyway.
    mlua_batteries::preload_all(&lua, mlua_batteries::PRELOAD_PREFIX).unwrap();

    let mut failures = Vec::new();
    for entry in dts::entries() {
        let declared = declared_fields(entry.source);
        let actual = module_keys(&lua, &build_module(&lua, entry.name));
        let missing: Vec<_> = actual.difference(&declared).cloned().collect();
        let extra: Vec<_> = declared.difference(&actual).cloned().collect();
        if !missing.is_empty() || !extra.is_empty() {
            failures.push(format!(
                "{name}.d.tl: not declared {missing:?}; declared but absent from the module {extra:?}",
                name = entry.name
            ));
        }
        assert!(
            !declared.is_empty(),
            "{}.d.tl declares no fields",
            entry.name
        );
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn every_declaration_has_a_module_and_vice_versa() {
    let declared: BTreeSet<&str> = dts::entries().into_iter().map(|e| e.name).collect();
    let modules: BTreeSet<&str> = mlua_batteries::module_entries()
        .into_iter()
        .map(|(n, _)| n)
        .chain(if cfg!(feature = "task") {
            Some("task")
        } else {
            None
        })
        .collect();
    // Until every module ships a declaration, only check the direction
    // that can silently rot: a declaration for a module that is gone.
    let orphaned: Vec<_> = declared.difference(&modules).collect();
    assert!(
        orphaned.is_empty(),
        "declarations without a module: {orphaned:?}"
    );
    let undeclared: Vec<_> = modules.difference(&declared).collect();
    if !undeclared.is_empty() {
        eprintln!("modules without a shipped declaration yet: {undeclared:?}");
    }
}

/// `htl check` over the written declarations, when htl is installed.
#[test]
fn declarations_type_check_under_htl() {
    let Ok(version) = Command::new("htl").arg("--version").output() else {
        eprintln!("htl not on PATH; skipping the Teal check of the declarations");
        return;
    };
    assert!(version.status.success());

    let dir = std::env::temp_dir().join(format!(
        "mlua-batteries-htl-check-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("htl.toml"), "").unwrap();
    dts::write_to(dir.join("types"), mlua_batteries::PRELOAD_PREFIX).unwrap();

    // A probe that requires every module and the namespace, so an invalid
    // declaration is an error at the require site.
    let mut probe = String::new();
    for e in dts::entries() {
        probe.push_str(&format!(
            "local {n} = require(\"{p}.{n}\")\nprint({n})\n",
            n = e.name,
            p = mlua_batteries::PRELOAD_PREFIX
        ));
    }
    probe.push_str(&format!(
        "local ns = require(\"{p}\")\nprint(ns)\n",
        p = mlua_batteries::PRELOAD_PREFIX
    ));
    // Call shapes a consumer actually writes, so a declaration that only
    // loads but cannot be used that way fails here.  Every function raises,
    // so the pcall forms are the ones that matter most.
    #[cfg(feature = "json")]
    probe.push_str(
        r#"
local record R
   name: string
end
local ok1, v1 = pcall(json.decode, "{}")
local r1: R = json.decode("{}")
local r2 = json.decode("{}")
local ok2, r3 = pcall(function(): R return json.decode("{}") end)
local ok3, f1 = pcall(json.read_file, "x.json")
local f2: R = json.read_file("x.json")
local e: R = { name = json.null as string }
local tags: {string} = json.array()
print(ok1, v1, r1, r2, ok2, r3, ok3, f1, f2, e, tags, json.is_null(json.null), json.encode(e))
"#,
    );
    std::fs::write(dir.join("src/probe.tl"), probe).unwrap();

    let out = Command::new("htl")
        .args(["check", "--no-cache", "src/probe.tl"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let report = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(out.status.success(), "htl check failed:\n{report}");
}
