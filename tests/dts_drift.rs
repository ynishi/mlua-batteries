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

/// `[package.metadata.htl] dts` in Cargo.toml lists the shipped files one
/// by one; it must name exactly the files in `types/mlua_batteries/`.
#[test]
fn cargo_metadata_lists_every_shipped_declaration() {
    let manifest =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    let start = manifest
        .find("[package.metadata.htl]")
        .expect("metadata.htl section");
    let section = &manifest[start..];
    let open = section.find("dts = [").expect("dts list") + "dts = [".len();
    let close = section[open..].find(']').expect("dts list end") + open;
    let listed: BTreeSet<String> = section[open..close]
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/types/mlua_batteries"));
    let on_disk: BTreeSet<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".d.tl"))
        .map(|n| format!("types/mlua_batteries/{n}"))
        .collect();
    assert_eq!(listed, on_disk);
}

/// Write the declarations for the prefix into a fresh temp project and
/// return its root, or `None` when htl is not installed.
fn htl_project() -> Option<std::path::PathBuf> {
    let Ok(version) = Command::new("htl").arg("--version").output() else {
        eprintln!("htl not on PATH; skipping the Teal check of the declarations");
        return None;
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
    Some(dir)
}

/// Run `htl check` on `src/probe.tl`; returns (success, combined output).
fn htl_check(dir: &std::path::Path, probe: &str, strict: bool) -> (bool, String) {
    std::fs::write(dir.join("src/probe.tl"), probe).unwrap();
    let mut args = vec!["check", "--no-cache"];
    if strict {
        args.push("--strict");
    }
    args.push("src/probe.tl");
    let out = Command::new("htl")
        .args(&args)
        .current_dir(dir)
        .output()
        .unwrap();
    let report = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), report)
}

/// Every declaration loads, and the call shapes a consumer actually writes
/// type-check with no error, warning or lint (`--strict`).
#[test]
fn declarations_type_check_under_htl() {
    let Some(dir) = htl_project() else { return };

    // `m_` so a module named like a Teal builtin (`string`) does not shadow it.
    let mut probe = String::new();
    for e in dts::entries() {
        probe.push_str(&format!(
            "local m_{n} = require(\"{p}.{n}\")\nprint(m_{n})\n",
            n = e.name,
            p = mlua_batteries::PRELOAD_PREFIX
        ));
    }
    probe.push_str(&format!(
        "local ns = require(\"{p}\")\nprint(ns)\n",
        p = mlua_batteries::PRELOAD_PREFIX
    ));

    // Every function raises, so the pcall forms matter most; the generic
    // `<T>` alone does not resolve through pcall, hence the `any` twin.
    #[cfg(feature = "json")]
    probe.push_str(
        r#"
local record R
   name: string
end
local ok1, v1 = pcall(m_json.decode, "{}")
local r1: R = m_json.decode("{}")
local r2 = m_json.decode("{}")
local ok2, r3 = pcall(function(): R return m_json.decode("{}") end)
local ok3, f1 = pcall(m_json.read_file, "x.json")
local f2: R = m_json.read_file("x.json")
local e: R = { name = m_json.null as string }
local tags: {string} = m_json.array()
local t2 = m_json.array()
print(ok1, v1, r1, r2, ok2, r3, ok3, f1, f2, e, tags, t2, m_json.is_null(m_json.null), m_json.encode(e))
"#,
    );
    // Optional trailing arguments (`name?: T`) left out, as the Rust side
    // allows.
    #[cfg(feature = "pretty")]
    probe.push_str("print(m_pretty.dump({ a = 1 }), m_pretty.dump({ a = 1 }, { indent = 0 }))\n");
    #[cfg(feature = "string")]
    probe.push_str(
        "print(m_string.pad_start(\"x\", 4), m_string.pad_end(\"x\", 4), m_string.truncate(\"abcdef\", 3))\n",
    );
    #[cfg(feature = "log")]
    probe.push_str("m_log.info(\"hi\")\nm_log.warn(\"hi\", { k = 1 })\n");
    #[cfg(feature = "http")]
    probe.push_str("local rq: m_http.Request = { method = \"GET\", url = \"u\" }\nprint(rq)\n");
    #[cfg(feature = "proc")]
    probe.push_str(
        "local st: m_proc.Stage = { argv = { \"ls\" } }\nlocal fr: m_proc.FileRef = { path = \"p\" }\nprint(st, fr)\n",
    );
    #[cfg(feature = "llm")]
    probe.push_str(
        "local lq: m_llm.Request = { provider = \"p\", model = \"m\", prompt = \"q\" }\nlocal lm: m_llm.Message = { role = \"user\", content = \"x\" }\nprint(lq, lm)\n",
    );
    #[cfg(feature = "argparse")]
    probe.push_str("local ps: m_argparse.Positional = { name = \"x\" }\nprint(ps)\n");
    // Nil-able returns are declared as the plain type (Teal has no non-nil
    // type); the ---@nilable marker is a comment to the checker.
    #[cfg(feature = "path")]
    probe.push_str("local par = m_path.parent(\"/a/b\")\nif par then print(par:upper()) end\n");

    let (ok, report) = htl_check(&dir, &probe, true);
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(ok, "htl check --strict failed:\n{report}");
}

/// `---@struct` records report a literal that leaves a required field out
/// (lint `struct-fields`), once per literal, naming the field.
#[test]
fn struct_markers_lint_missing_required_fields() {
    let Some(dir) = htl_project() else { return };

    let mut probe = String::new();
    let mut expected: Vec<&str> = Vec::new();
    #[cfg(feature = "argparse")]
    {
        probe.push_str("local m_argparse = require(\"mlua_batteries.argparse\")\nlocal a: m_argparse.Positional = { required = true }\nprint(a)\n");
        expected.push("Positional is built without name");
    }
    #[cfg(feature = "http")]
    {
        probe.push_str("local m_http = require(\"mlua_batteries.http\")\nlocal b: m_http.Request = { url = \"u\" }\nprint(b)\n");
        expected.push("Request is built without method");
    }
    #[cfg(feature = "proc")]
    {
        probe.push_str("local m_proc = require(\"mlua_batteries.proc\")\nlocal c: m_proc.Stage = { env = {} }\nlocal d: m_proc.FileRef = { append = true }\nprint(c, d)\n");
        expected.push("Stage is built without argv");
        expected.push("FileRef is built without path");
    }
    #[cfg(feature = "llm")]
    {
        probe.push_str("local m_llm = require(\"mlua_batteries.llm\")\nlocal e: m_llm.Request = { model = \"m\" }\nlocal f: m_llm.Message = { role = \"user\" }\nprint(e, f)\n");
        expected.push("Request is built without provider");
        expected.push("Message is built without content");
    }
    if expected.is_empty() {
        std::fs::remove_dir_all(&dir).unwrap();
        return;
    }

    let (_, report) = htl_check(&dir, &probe, false);
    std::fs::remove_dir_all(&dir).unwrap();
    let lints: Vec<&str> = report
        .lines()
        .filter(|l| l.contains("[htl struct-fields]"))
        .collect();
    assert_eq!(lints.len(), expected.len(), "{report}");
    for want in expected {
        assert!(
            lints.iter().any(|l| l.contains(want)),
            "missing `{want}` in:\n{report}"
        );
    }
    assert!(report.contains(" 0 error(s)"), "{report}");
}
