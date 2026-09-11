//! Command-line argument parsing from a spec table — `std.argparse`.
//!
//! ```lua
//! local argparse = std.argparse
//! local spec = {
//!     name = "cardbox",
//!     flags = {
//!         json    = { type = "boolean", short = "j", help = "print JSON" },
//!         port    = { type = "integer", default = 8080 },
//!         include = { type = "string", multiple = true, short = "I" },
//!     },
//!     positionals = {
//!         { name = "command", required = true },
//!         { name = "files", rest = true },
//!     },
//! }
//! local r = argparse.parse(arg, spec)
//! -- r.opts.json      true / false / nil
//! -- r.opts.port      8080 unless --port was given
//! -- r.opts.include   { "a", "b" } for -I a -I b (empty table when absent)
//! -- r.args.command   "list"
//! -- r.args.files     { "x.md", "y.md" }
//! print(argparse.usage(spec))
//! ```
//!
//! The spec is data, so a host-specific convention (a `--json` every
//! command accepts, say) is one entry in the host's spec rather than a
//! feature of this module.
//!
//! # Accepted forms
//!
//! - `--name value`, `--name=value`, `-n value`, `-nvalue`
//! - booleans: `--flag`, `--no-flag`, `-f`; bundled shorts `-vq` when
//!   every letter is a boolean flag; `--flag=true|false` also works
//! - `--some-name` and `--some_name` both address the flag `some_name`
//! - `--` ends option parsing; everything after it is positional
//! - a value that starts with `-` is only taken for an `integer` /
//!   `number` flag when it parses as one (`--offset -3`); a string flag
//!   given `--name --other` raises instead of swallowing the next option
//!
//! # Results
//!
//! `parse` returns `{ opts, args, rest }`.  `opts` holds every flag by
//! name: the parsed value, the `default` when absent, `{}` for an absent
//! `multiple` flag, and nil otherwise (so `if r.opts.verbose then` reads
//! naturally).  `args` holds the declared positionals by name, the `rest`
//! positional as a list.  `rest` is what the spec did not claim — unknown
//! options and surplus positionals — and is only ever non-empty when
//! `allow_unknown = true`; without it those raise.
//!
//! # Errors
//!
//! Everything raises, with an `argparse:` prefix: an unknown option, a
//! value of the wrong type, a missing required flag or positional, a
//! surplus positional, and a malformed spec (unknown `type`, a `short`
//! that is not one character, two flags sharing a short, `rest` on a
//! positional that is not last, `required` together with `default`).
//!
//! # Not covered
//!
//! Subcommands (parse the first positional and dispatch to a second
//! spec), environment-variable fallbacks, and an automatic `--help`
//! (declare a boolean flag and print `usage(spec)` yourself).

use std::collections::{BTreeMap, HashMap};

use mlua::prelude::*;

// ─── Spec ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    String,
    Integer,
    Number,
    Boolean,
}

impl Kind {
    fn parse(name: &str, ctx: &str) -> LuaResult<Self> {
        Ok(match name {
            "string" => Kind::String,
            "integer" => Kind::Integer,
            "number" => Kind::Number,
            "boolean" => Kind::Boolean,
            other => {
                return Err(err(format!(
                    "{ctx}: unknown type \"{other}\" (expected string, integer, number or boolean)"
                )))
            }
        })
    }

    fn label(self) -> &'static str {
        match self {
            Kind::String => "string",
            Kind::Integer => "integer",
            Kind::Number => "number",
            Kind::Boolean => "boolean",
        }
    }
}

#[derive(Debug)]
struct Flag {
    name: String,
    kind: Kind,
    short: Option<char>,
    default: LuaValue,
    required: bool,
    multiple: bool,
    help: Option<String>,
    metavar: Option<String>,
}

#[derive(Debug)]
struct Positional {
    name: String,
    kind: Kind,
    required: bool,
    rest: bool,
    help: Option<String>,
}

#[derive(Debug)]
struct Spec {
    name: Option<String>,
    description: Option<String>,
    /// Sorted by name, so usage output and error messages are stable.
    flags: BTreeMap<String, Flag>,
    shorts: HashMap<char, String>,
    positionals: Vec<Positional>,
    allow_unknown: bool,
}

fn err(msg: impl Into<String>) -> LuaError {
    LuaError::external(format!("argparse: {}", msg.into()))
}

fn opt_string(t: &LuaTable, key: &str, ctx: &str) -> LuaResult<Option<String>> {
    match t.get::<LuaValue>(key)? {
        LuaValue::Nil => Ok(None),
        LuaValue::String(s) => Ok(Some(s.to_str()?.to_string())),
        other => Err(err(format!(
            "{ctx}: {key} must be a string, got {}",
            other.type_name()
        ))),
    }
}

fn opt_bool(t: &LuaTable, key: &str, ctx: &str) -> LuaResult<bool> {
    match t.get::<LuaValue>(key)? {
        LuaValue::Nil => Ok(false),
        LuaValue::Boolean(b) => Ok(b),
        other => Err(err(format!(
            "{ctx}: {key} must be a boolean, got {}",
            other.type_name()
        ))),
    }
}

impl Spec {
    fn from_lua(t: &LuaTable) -> LuaResult<Self> {
        let name = opt_string(t, "name", "spec")?;
        let description = opt_string(t, "description", "spec")?;
        let allow_unknown = opt_bool(t, "allow_unknown", "spec")?;

        let mut flags = BTreeMap::new();
        let mut shorts = HashMap::new();
        if let Some(ft) = t.get::<Option<LuaTable>>("flags")? {
            for pair in ft.pairs::<LuaValue, LuaTable>() {
                let (key, def) = pair.map_err(|e| err(format!("spec.flags: {e}")))?;
                let LuaValue::String(key) = key else {
                    return Err(err(format!(
                        "spec.flags: keys must be flag names, got {}",
                        key.type_name()
                    )));
                };
                let name = key.to_str()?.to_string();
                let ctx = format!("spec.flags.{name}");
                if !is_flag_name(&name) {
                    return Err(err(format!(
                        "{ctx}: flag names are letters, digits and _ (not starting with a digit)"
                    )));
                }
                let kind = Kind::parse(
                    &opt_string(&def, "type", &ctx)?.unwrap_or_else(|| "string".into()),
                    &ctx,
                )?;
                let short = match opt_string(&def, "short", &ctx)? {
                    None => None,
                    Some(s) => {
                        let mut it = s.chars();
                        match (it.next(), it.next()) {
                            (Some(c), None) if c.is_ascii_alphanumeric() => Some(c),
                            _ => {
                                return Err(err(format!(
                                    "{ctx}: short must be one letter or digit, got \"{s}\""
                                )))
                            }
                        }
                    }
                };
                if let Some(c) = short {
                    if let Some(prev) = shorts.insert(c, name.clone()) {
                        return Err(err(format!("{ctx}: short -{c} is already used by {prev}")));
                    }
                }
                let default: LuaValue = def.get("default")?;
                let required = opt_bool(&def, "required", &ctx)?;
                let multiple = opt_bool(&def, "multiple", &ctx)?;
                if required && !default.is_nil() {
                    return Err(err(format!("{ctx}: required and default are exclusive")));
                }
                if kind == Kind::Boolean && multiple {
                    return Err(err(format!("{ctx}: a boolean flag cannot be multiple")));
                }
                flags.insert(
                    name.clone(),
                    Flag {
                        name,
                        kind,
                        short,
                        default,
                        required,
                        multiple,
                        help: opt_string(&def, "help", &ctx)?,
                        metavar: opt_string(&def, "metavar", &ctx)?,
                    },
                );
            }
        }

        let mut positionals = Vec::new();
        if let Some(pt) = t.get::<Option<LuaTable>>("positionals")? {
            let n = pt.raw_len();
            for i in 1..=n {
                let def: LuaTable = pt.raw_get(i)?;
                let ctx = format!("spec.positionals[{i}]");
                let name = opt_string(&def, "name", &ctx)?
                    .ok_or_else(|| err(format!("{ctx}: name is required")))?;
                let kind = Kind::parse(
                    &opt_string(&def, "type", &ctx)?.unwrap_or_else(|| "string".into()),
                    &ctx,
                )?;
                if kind == Kind::Boolean {
                    return Err(err(format!("{ctx}: a positional cannot be boolean")));
                }
                let rest = opt_bool(&def, "rest", &ctx)?;
                if rest && i != n {
                    return Err(err(format!("{ctx}: rest must be the last positional")));
                }
                positionals.push(Positional {
                    name,
                    kind,
                    required: opt_bool(&def, "required", &ctx)?,
                    rest,
                    help: opt_string(&def, "help", &ctx)?,
                });
            }
        }

        Ok(Spec {
            name,
            description,
            flags,
            shorts,
            positionals,
            allow_unknown,
        })
    }

    /// `--some-name` and `--some_name` both address `some_name`.
    fn flag_by_long(&self, long: &str) -> Option<&Flag> {
        self.flags.get(&long.replace('-', "_"))
    }

    fn flag_by_short(&self, c: char) -> Option<&Flag> {
        self.shorts.get(&c).and_then(|n| self.flags.get(n))
    }
}

fn is_flag_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// ─── Values ───────────────────────────────────────────

fn convert(lua: &Lua, kind: Kind, raw: &str, what: &str) -> LuaResult<LuaValue> {
    Ok(match kind {
        Kind::String => LuaValue::String(lua.create_string(raw)?),
        Kind::Integer => match raw.parse::<i64>() {
            Ok(i) => LuaValue::Integer(i),
            Err(_) => return Err(err(format!("{what} expects an integer, got \"{raw}\""))),
        },
        Kind::Number => match raw.parse::<f64>() {
            Ok(n) if n.is_finite() => LuaValue::Number(n),
            _ => return Err(err(format!("{what} expects a number, got \"{raw}\""))),
        },
        Kind::Boolean => match raw {
            "true" | "yes" | "1" | "on" => LuaValue::Boolean(true),
            "false" | "no" | "0" | "off" => LuaValue::Boolean(false),
            _ => return Err(err(format!("{what} expects true or false, got \"{raw}\""))),
        },
    })
}

/// Can `token` be taken as the value of a flag of this kind?  A token
/// that starts with `-` is only a value when the flag is numeric and the
/// token parses as a number.
fn takes_as_value(kind: Kind, token: &str) -> bool {
    if !token.starts_with('-') || token == "-" {
        return true;
    }
    match kind {
        Kind::Integer => token.parse::<i64>().is_ok(),
        Kind::Number => token.parse::<f64>().is_ok(),
        _ => false,
    }
}

// ─── Parse ────────────────────────────────────────────

struct Parsed {
    opts: HashMap<String, LuaValue>,
    positional: Vec<String>,
    rest: Vec<String>,
}

impl Parsed {
    fn set(&mut self, lua: &Lua, flag: &Flag, value: LuaValue) -> LuaResult<()> {
        if flag.multiple {
            let list = match self.opts.get(&flag.name) {
                Some(LuaValue::Table(t)) => t.clone(),
                _ => {
                    let t = lua.create_table()?;
                    self.opts
                        .insert(flag.name.clone(), LuaValue::Table(t.clone()));
                    t
                }
            };
            list.raw_push(value)?;
        } else {
            self.opts.insert(flag.name.clone(), value);
        }
        Ok(())
    }
}

fn parse_argv(lua: &Lua, spec: &Spec, argv: &[String]) -> LuaResult<Parsed> {
    let mut out = Parsed {
        opts: HashMap::new(),
        positional: Vec::new(),
        rest: Vec::new(),
    };
    let mut i = 0;
    let mut only_positional = false;

    while i < argv.len() {
        let tok = argv[i].as_str();
        i += 1;

        if only_positional || tok == "-" || !tok.starts_with('-') {
            out.positional.push(tok.to_string());
            continue;
        }
        if tok == "--" {
            only_positional = true;
            continue;
        }

        if let Some(long) = tok.strip_prefix("--") {
            let (key, inline) = match long.split_once('=') {
                Some((k, v)) => (k, Some(v)),
                None => (long, None),
            };
            // --no-flag for booleans.
            if inline.is_none() {
                if let Some(base) = key.strip_prefix("no-") {
                    if let Some(flag) = spec.flag_by_long(base) {
                        if flag.kind == Kind::Boolean {
                            out.set(lua, flag, LuaValue::Boolean(false))?;
                            continue;
                        }
                    }
                }
            }
            let Some(flag) = spec.flag_by_long(key) else {
                if spec.allow_unknown {
                    out.rest.push(tok.to_string());
                    continue;
                }
                return Err(err(format!("unknown option --{key}")));
            };
            let what = format!("--{key}");
            let value = match (flag.kind, inline) {
                (Kind::Boolean, None) => LuaValue::Boolean(true),
                (kind, Some(v)) => convert(lua, kind, v, &what)?,
                (kind, None) => {
                    let next = argv.get(i).map(String::as_str);
                    match next {
                        Some(v) if takes_as_value(kind, v) => {
                            i += 1;
                            convert(lua, kind, v, &what)?
                        }
                        _ => return Err(err(format!("{what} expects a value"))),
                    }
                }
            };
            out.set(lua, flag, value)?;
            continue;
        }

        // Short option(s): -v, -vq, -n value, -nvalue.
        let body = &tok[1..];
        for (pos, c) in body.char_indices() {
            let Some(flag) = spec.flag_by_short(c) else {
                if spec.allow_unknown && pos == 0 {
                    out.rest.push(tok.to_string());
                    break;
                }
                return Err(err(format!("unknown option -{c}")));
            };
            let what = format!("-{c}");
            if flag.kind == Kind::Boolean {
                out.set(lua, flag, LuaValue::Boolean(true))?;
                continue;
            }
            let attached = &body[pos + c.len_utf8()..];
            let value = if !attached.is_empty() {
                convert(lua, flag.kind, attached, &what)?
            } else {
                match argv.get(i).map(String::as_str) {
                    Some(v) if takes_as_value(flag.kind, v) => {
                        i += 1;
                        convert(lua, flag.kind, v, &what)?
                    }
                    _ => return Err(err(format!("{what} expects a value"))),
                }
            };
            out.set(lua, flag, value)?;
            break;
        }
    }

    Ok(out)
}

fn build_result(lua: &Lua, spec: &Spec, mut parsed: Parsed) -> LuaResult<LuaTable> {
    let opts = lua.create_table()?;
    for flag in spec.flags.values() {
        match parsed.opts.remove(&flag.name) {
            Some(v) => opts.set(flag.name.as_str(), v)?,
            None if flag.required => {
                return Err(err(format!(
                    "missing required option --{}",
                    dashed(&flag.name)
                )))
            }
            None if !flag.default.is_nil() => opts.set(flag.name.as_str(), flag.default.clone())?,
            None if flag.multiple => opts.set(flag.name.as_str(), lua.create_table()?)?,
            None => {}
        }
    }

    let args = lua.create_table()?;
    let mut tokens = parsed.positional.into_iter();
    for pos in &spec.positionals {
        if pos.rest {
            let list = lua.create_table()?;
            for tok in tokens.by_ref() {
                list.raw_push(convert(lua, pos.kind, &tok, &pos.name)?)?;
            }
            args.set(pos.name.as_str(), list)?;
            continue;
        }
        match tokens.next() {
            Some(tok) => args.set(pos.name.as_str(), convert(lua, pos.kind, &tok, &pos.name)?)?,
            None if pos.required => {
                return Err(err(format!("missing required argument <{}>", pos.name)))
            }
            None => {}
        }
    }
    let surplus: Vec<String> = tokens.collect();
    if !surplus.is_empty() {
        if !spec.allow_unknown {
            return Err(err(format!("unexpected argument \"{}\"", surplus[0])));
        }
        parsed.rest.extend(surplus);
    }

    let rest = lua.create_table()?;
    for tok in parsed.rest {
        rest.raw_push(tok)?;
    }

    let result = lua.create_table()?;
    result.set("opts", opts)?;
    result.set("args", args)?;
    result.set("rest", rest)?;
    Ok(result)
}

/// `some_name` → `some-name`, the spelling usage shows.
fn dashed(name: &str) -> String {
    name.replace('_', "-")
}

// ─── Usage ────────────────────────────────────────────

fn usage(spec: &Spec) -> String {
    let mut out = String::new();
    out.push_str("Usage: ");
    out.push_str(spec.name.as_deref().unwrap_or("<program>"));
    if !spec.flags.is_empty() {
        out.push_str(" [options]");
    }
    for pos in &spec.positionals {
        out.push(' ');
        let inner = if pos.rest {
            format!("{}...", pos.name)
        } else {
            pos.name.clone()
        };
        if pos.required {
            out.push_str(&format!("<{inner}>"));
        } else {
            out.push_str(&format!("[{inner}]"));
        }
    }
    out.push('\n');
    if let Some(d) = &spec.description {
        out.push('\n');
        out.push_str(d);
        out.push('\n');
    }

    let positional_rows: Vec<(String, String)> = spec
        .positionals
        .iter()
        .map(|p| {
            let mut help = p.help.clone().unwrap_or_default();
            if p.kind != Kind::String {
                help = append_note(help, p.kind.label());
            }
            (p.name.clone(), help)
        })
        .collect();
    let flag_rows: Vec<(String, String)> = spec
        .flags
        .values()
        .map(|f| {
            let short = match f.short {
                Some(c) => format!("-{c}, "),
                None => "    ".to_string(),
            };
            let mut left = format!("{short}--{}", dashed(&f.name));
            if f.kind != Kind::Boolean {
                let metavar = f
                    .metavar
                    .clone()
                    .unwrap_or_else(|| f.kind.label().to_string());
                left.push_str(&format!(" <{metavar}>"));
            }
            let mut help = f.help.clone().unwrap_or_default();
            if f.required {
                help = append_note(help, "required");
            } else if !f.default.is_nil() {
                let shown = match &f.default {
                    LuaValue::String(s) => s.to_string_lossy(),
                    other => other.to_string().unwrap_or_default(),
                };
                help = append_note(help, &format!("default: {shown}"));
            }
            if f.multiple {
                help = append_note(help, "repeatable");
            }
            (left, help)
        })
        .collect();

    let width = positional_rows
        .iter()
        .chain(flag_rows.iter())
        .map(|(l, _)| l.chars().count())
        .max()
        .unwrap_or(0);

    if !positional_rows.is_empty() {
        out.push_str("\nArguments:\n");
        for (l, h) in &positional_rows {
            push_row(&mut out, l, h, width);
        }
    }
    if !flag_rows.is_empty() {
        out.push_str("\nOptions:\n");
        for (l, h) in &flag_rows {
            push_row(&mut out, l, h, width);
        }
    }
    out
}

fn append_note(help: String, note: &str) -> String {
    if help.is_empty() {
        format!("({note})")
    } else {
        format!("{help} ({note})")
    }
}

fn push_row(out: &mut String, left: &str, help: &str, width: usize) {
    out.push_str("  ");
    out.push_str(left);
    if !help.is_empty() {
        let pad = width - left.chars().count() + 2;
        out.extend(std::iter::repeat_n(' ', pad));
        out.push_str(help);
    }
    out.push('\n');
}

// ─── Module ───────────────────────────────────────────

pub fn module(lua: &Lua) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;

    t.set(
        "parse",
        lua.create_function(|lua, (argv, spec): (LuaTable, LuaTable)| {
            let spec = Spec::from_lua(&spec)?;
            let mut args = Vec::with_capacity(argv.raw_len());
            for i in 1..=argv.raw_len() {
                match argv.raw_get::<LuaValue>(i)? {
                    LuaValue::String(s) => args.push(s.to_str()?.to_string()),
                    other => {
                        return Err(err(format!(
                            "argv[{i}] must be a string, got {}",
                            other.type_name()
                        )))
                    }
                }
            }
            let parsed = parse_argv(lua, &spec, &args)?;
            build_result(lua, &spec, parsed)
        })?,
    )?;

    t.set(
        "usage",
        lua.create_function(|_, spec: LuaTable| {
            let spec = Spec::from_lua(&spec)?;
            Ok(usage(&spec))
        })?,
    )?;

    Ok(t)
}

#[cfg(test)]
mod tests {
    use mlua::prelude::*;

    fn run(code: &str) -> LuaResult<String> {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();
        lua.load(format!(
            r#"
            local argparse = std.argparse
            local spec = {{
                name = "tool",
                flags = {{
                    json    = {{ type = "boolean", short = "j", help = "print JSON" }},
                    verbose = {{ type = "boolean", short = "v" }},
                    quiet   = {{ type = "boolean", short = "q" }},
                    port    = {{ type = "integer", default = 8080, short = "p" }},
                    ratio   = {{ type = "number" }},
                    name    = {{ type = "string", required = true, short = "n" }},
                    include = {{ type = "string", multiple = true, short = "I", metavar = "DIR" }},
                    dry_run = {{ type = "boolean" }},
                }},
                positionals = {{
                    {{ name = "command", required = true }},
                    {{ name = "count", type = "integer" }},
                    {{ name = "files", rest = true }},
                }},
            }}
            {code}
            "#
        ))
        .eval::<String>()
    }

    fn ok(code: &str) -> String {
        run(code).unwrap()
    }

    fn fails(code: &str) -> String {
        run(code).unwrap_err().to_string()
    }

    #[test]
    fn long_short_inline_and_attached_values() {
        let s = ok(r#"
            local r = argparse.parse({ "-n", "x", "list", "--port=9", "-I", "a", "-Ib", "--ratio", "0.5", "--dry-run", "--json" }, spec)
            return std.pretty.dump(r, { indent = 0 })
        "#);
        assert_eq!(
            s,
            r#"{ args = { command = "list", files = {} }, opts = { dry_run = true, include = { "a", "b" }, json = true, name = "x", port = 9, ratio = 0.5 }, rest = {} }"#
        );
    }

    #[test]
    fn defaults_absent_multiple_and_positionals() {
        let s = ok(r#"
            local r = argparse.parse({ "run", "-n", "x", "3", "f1", "f2" }, spec)
            return std.pretty.dump(r, { indent = 0 })
        "#);
        assert_eq!(
            s,
            r#"{ args = { command = "run", count = 3, files = { "f1", "f2" } }, opts = { include = {}, name = "x", port = 8080 }, rest = {} }"#
        );
    }

    #[test]
    fn booleans_no_prefix_bundle_and_explicit_value() {
        let s = ok(r#"
            local r = argparse.parse({ "-vq", "--no-json", "--dry_run=false", "-n", "x", "c" }, spec)
            return tostring(r.opts.verbose) .. tostring(r.opts.quiet) .. tostring(r.opts.json) .. tostring(r.opts.dry_run)
        "#);
        assert_eq!(s, "truetruefalsefalse");
    }

    #[test]
    fn double_dash_ends_options_and_negative_numbers_are_values() {
        let s = ok(r#"
            local r = argparse.parse({ "-n", "x", "--port", "-1", "c", "2", "--", "--not-a-flag", "-v" }, spec)
            return std.pretty.dump({ r.opts.port, r.args.files }, { indent = 0 })
        "#);
        assert_eq!(s, r#"{ -1, { "--not-a-flag", "-v" } }"#);
    }

    #[test]
    fn errors_name_the_problem() {
        let cases: &[(&str, &str)] = &[
            (
                r#"argparse.parse({ "--nope", "c" }, spec)"#,
                "unknown option --nope",
            ),
            (
                r#"argparse.parse({ "-z", "c" }, spec)"#,
                "unknown option -z",
            ),
            (
                r#"argparse.parse({ "-n", "x", "--port", "abc", "c" }, spec)"#,
                "--port expects an integer, got \"abc\"",
            ),
            (
                r#"argparse.parse({ "-n", "x", "--port", "c" }, spec)"#,
                "--port expects an integer, got \"c\"",
            ),
            (
                r#"argparse.parse({ "-n", "--json", "c" }, spec)"#,
                "-n expects a value",
            ),
            (
                r#"argparse.parse({ "-n", "x", "--ratio" }, spec)"#,
                "--ratio expects a value",
            ),
            (
                r#"argparse.parse({ "c" }, spec)"#,
                "missing required option --name",
            ),
            (
                r#"argparse.parse({ "-n", "x" }, spec)"#,
                "missing required argument <command>",
            ),
            (
                r#"argparse.parse({ "-n", "x", "c", "zz" }, spec)"#,
                "count expects an integer, got \"zz\"",
            ),
            (
                r#"argparse.parse({ "-n", "x", "c", "-j", 1 }, spec)"#,
                "argv[5] must be a string",
            ),
        ];
        for (code, expected) in cases {
            let msg = fails(&format!("return tostring({code})"));
            assert!(
                msg.contains(expected),
                "{code}\n  got: {msg}\n  want: {expected}"
            );
        }
    }

    #[test]
    fn surplus_positional_raises_unless_allow_unknown() {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let strict: LuaResult<LuaValue> = lua
            .load(r#"return std.argparse.parse({ "a", "b" }, { positionals = { { name = "one" } } })"#)
            .eval();
        assert!(strict
            .unwrap_err()
            .to_string()
            .contains("unexpected argument \"b\""));
        let lenient: String = lua
            .load(
                r#"
                local r = std.argparse.parse({ "a", "--x", "b", "-y" }, { allow_unknown = true, positionals = { { name = "one" } } })
                return std.pretty.dump(r, { indent = 0 })
            "#,
            )
            .eval()
            .unwrap();
        assert_eq!(
            s(lenient),
            r#"{ args = { one = "a" }, opts = {}, rest = { "--x", "-y", "b" } }"#
        );
        fn s(x: String) -> String {
            x
        }
    }

    #[test]
    fn spec_validation() {
        let cases: &[(&str, &str)] = &[
            (
                r#"{ flags = { a = { type = "list" } } }"#,
                "unknown type \"list\"",
            ),
            (
                r#"{ flags = { a = { short = "ab" } } }"#,
                "short must be one letter",
            ),
            (
                r#"{ flags = { a = { short = "x" }, b = { short = "x" } } }"#,
                "short -x is already used by",
            ),
            (
                r#"{ flags = { a = { required = true, default = 1 } } }"#,
                "required and default are exclusive",
            ),
            (
                r#"{ flags = { a = { type = "boolean", multiple = true } } }"#,
                "cannot be multiple",
            ),
            (
                r#"{ flags = { ["bad-name"] = {} } }"#,
                "flag names are letters",
            ),
            (
                r#"{ positionals = { { name = "a", rest = true }, { name = "b" } } }"#,
                "rest must be the last",
            ),
            (
                r#"{ positionals = { { rest = true } } }"#,
                "name is required",
            ),
            (
                r#"{ positionals = { { name = "a", type = "boolean" } } }"#,
                "cannot be boolean",
            ),
        ];
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();
        for (spec, expected) in cases {
            let r: LuaResult<LuaValue> = lua
                .load(format!("return std.argparse.usage({spec})"))
                .eval();
            let msg = r.unwrap_err().to_string();
            assert!(msg.contains(expected), "{spec}\n  got: {msg}");
        }
    }

    #[test]
    fn usage_layout() {
        let s = ok("return argparse.usage(spec)");
        let expected = "\
Usage: tool [options] <command> [count] [files...]

Arguments:
  command
  count                 (integer)
  files

Options:
      --dry-run
  -I, --include <DIR>   (repeatable)
  -j, --json            print JSON
  -n, --name <string>   (required)
  -p, --port <integer>  (default: 8080)
  -q, --quiet
      --ratio <number>
  -v, --verbose
";
        assert_eq!(s, expected);
    }
}
