//! Batteries-included standard library modules for mlua.
//!
//! Each module exposes a single `module(lua) -> LuaResult<LuaTable>` entry point.
//! Register individually or use [`register_all`] for convenience.
//!
//! # Platform support
//!
//! This crate targets **Unix server platforms** (Linux, macOS).
//! Windows is not a supported target.
//!
//! # Encoding — UTF-8 only (by design)
//!
//! All path arguments are received as Rust [`String`] (UTF-8).
//! Non-UTF-8 Lua strings are rejected at the `FromLua` boundary.
//! Returned paths use [`to_string_lossy`](std::path::Path::to_string_lossy),
//! replacing any non-UTF-8 bytes with U+FFFD.
//!
//! ## Why not raw bytes / `OsStr`?
//!
//! mlua's `FromLua` for `String` performs UTF-8 validation — non-UTF-8
//! values produce `FromLuaConversionError` before reaching handler code.
//! Bypassing this would require accepting `mlua::String` + `as_bytes()`
//! in every function, converting through `OsStr::from_bytes()`, and
//! returning `OsStr::as_bytes()` back to Lua.  This adds complexity
//! across all path-accepting functions for a scenario (non-UTF-8
//! filenames) that is rare on modern systems.
//!
//! References:
//! - mlua `String::to_str()`: <https://docs.rs/mlua/latest/mlua/struct.String.html>
//! - mlua string internals: <https://deepwiki.com/mlua-rs/mlua/2.3.4-strings>
//!
//! # Quick start
//!
//! ```rust,no_run
//! use mlua::prelude::*;
//!
//! let lua = Lua::new();
//! mlua_batteries::register_all(&lua, "std").unwrap();
//! // Lua: std.json.encode({a = 1})
//! // Lua: std.env.get("HOME")
//! ```
//!
//! # `require` instead of a global (Teal / htl)
//!
//! [`register_all`] installs a global table, which is the convenient
//! shape for plain Lua.  A Teal project (htl lints `global` away) reaches
//! the same modules through `require`: [`preload_all`] registers every
//! enabled module in `package.preload` under `<prefix>.<name>`, plus the
//! namespace itself under `<prefix>`, and touches no global.  The
//! declarations that let the Teal checker see them are in
//! [`dts`](crate::dts), written to a project's `types/` with the same
//! prefix.
//!
//! ```rust,no_run
//! use mlua::prelude::*;
//!
//! let lua = Lua::new();
//! mlua_batteries::preload_all(&lua, mlua_batteries::PRELOAD_PREFIX).unwrap();
//! // Lua / Teal: local json = require("mlua_batteries.json")
//! //             local std  = require("mlua_batteries")   -- every module in one table
//! ```
//!
//! # Async
//!
//! The modules above are synchronous and need no runtime.  Two opt-in
//! pieces are async, and both want a tokio current-thread runtime driving
//! a `LocalSet`:
//!
//! - `task` — structured concurrency primitives (`std.task.*`).
//! - `async_overrides` — replaces the blocking entries of an
//!   already-registered namespace (`std.time.sleep`, `std.proc.pipeline`,
//!   `std.http.*`, `std.fs.*`) with async ones, so they no longer park the
//!   VM thread.  Same Lua-side API; opt in by calling it after
//!   [`register_all`].
//!
//! # Custom configuration
//!
//! ```rust,ignore
//! // Requires the `sandbox` feature.
//! use mlua::prelude::*;
//! use mlua_batteries::config::Config;
//! use mlua_batteries::policy::Sandboxed;
//!
//! let lua = Lua::new();
//! let config = Config::builder()
//!     .path_policy(Sandboxed::new(["/app/data"]).unwrap().read_only())
//!     .max_walk_depth(50)
//!     .build()
//!     .expect("invalid config");
//! mlua_batteries::register_all_with(&lua, "std", config).unwrap();
//! ```

pub mod config;
pub mod dts;
pub mod policy;

#[cfg(feature = "task")]
pub mod async_overrides;
#[cfg(feature = "base64")]
pub mod base64;
#[cfg(feature = "env")]
pub mod env;
#[cfg(feature = "fs")]
pub mod fs;
#[cfg(feature = "hash")]
pub mod hash;
#[cfg(feature = "http")]
pub mod http;
#[cfg(feature = "json")]
pub mod json;
#[cfg(feature = "llm")]
pub mod llm;
#[cfg(feature = "log")]
pub mod log;
#[cfg(feature = "path")]
pub mod path;
#[cfg(feature = "pretty")]
pub mod pretty;
#[cfg(feature = "proc")]
pub mod proc;
#[cfg(feature = "regex")]
pub mod regex;
#[cfg(feature = "string")]
pub mod string;
#[cfg(feature = "task")]
pub mod task;
#[cfg(feature = "time")]
pub mod time;
#[cfg(feature = "uuid")]
pub mod uuid;
#[cfg(feature = "validate")]
pub mod validate;
#[cfg(feature = "watch")]
pub mod watch;

pub(crate) mod util;

use config::Config;
use mlua::prelude::*;

/// Module factory function type.
pub type ModuleFactory = fn(&Lua) -> LuaResult<LuaTable>;

/// Register all enabled modules with default configuration.
///
/// Equivalent to `register_all_with(lua, namespace, Config::default())`.
///
/// # Warning
///
/// The default configuration uses [`policy::Unrestricted`], which allows
/// Lua scripts to access **any** file on the filesystem.  For untrusted
/// scripts, use [`register_all_with`] with a [`policy::Sandboxed`] policy.
pub fn register_all(lua: &Lua, namespace: &str) -> LuaResult<LuaTable> {
    register_all_with(lua, namespace, Config::default())
}

/// Register all enabled modules with custom configuration.
///
/// The [`Config`] is stored in `lua.app_data` and consulted by each
/// module for policy checks and limit values.
///
/// # Calling multiple times
///
/// Calling this function again on the same [`Lua`] instance **replaces**
/// the previous [`Config`] (and the shared HTTP agent, if the `http`
/// feature is enabled).  Functions registered by earlier calls remain
/// in the namespace table but will use the **new** Config for all
/// subsequent invocations.  This is intentional — it allows
/// reconfiguration — but callers should be aware that there is no
/// "merge" behaviour.
pub fn register_all_with(lua: &Lua, namespace: &str, config: Config) -> LuaResult<LuaTable> {
    lua.set_app_data(config);

    let ns = lua.create_table()?;

    macro_rules! register {
        ($name:literal, $mod:ident) => {{
            #[cfg(feature = $name)]
            ns.set($name, $mod::module(lua)?)?;
        }};
    }

    register!("json", json);
    register!("env", env);
    register!("path", path);
    register!("string", string);
    register!("regex", regex);
    register!("validate", validate);
    register!("pretty", pretty);
    register!("log", log);
    register!("uuid", uuid);
    register!("base64", base64);
    register!("time", time);
    register!("fs", fs);
    register!("http", http);
    register!("llm", llm);
    register!("hash", hash);
    register!("proc", proc);
    register!("watch", watch);

    lua.globals().set(namespace, ns.clone())?;
    Ok(ns)
}

/// The `require` prefix this crate's shipped Teal declarations are named
/// under (`require("mlua_batteries.json")`), and the one to pass
/// [`preload_all`] unless the host composes its own namespace.
///
/// It is the crate's own name rather than `std` on purpose: `std` is the
/// host's namespace to assemble (a host may keep `std.fs` for its own
/// sandboxed module and take only `std.json` from here), so nothing this
/// crate ships claims it.
pub const PRELOAD_PREFIX: &str = "mlua_batteries";

/// Register every enabled module in `package.preload` with default
/// configuration.
///
/// Equivalent to `preload_all_with(lua, prefix, Config::default())`; the
/// warning on [`register_all`] about the unrestricted default policy
/// applies here too.
pub fn preload_all(lua: &Lua, prefix: &str) -> LuaResult<()> {
    preload_all_with(lua, prefix, Config::default())
}

/// Register every enabled module in `package.preload` with custom
/// configuration.
///
/// After this call `require("<prefix>.json")` (and so on for each module
/// in [`module_entries`], plus `task` when that feature is on) returns
/// the module table, and
/// `require("<prefix>")` returns a namespace table holding all of them —
/// the same table instances, since the namespace loader goes through
/// `require` itself.  Modules are built lazily on first `require` and
/// cached by `package.loaded` as usual.  No global is set, which is what a
/// Teal / htl project wants (see the crate docs); the two entry points are
/// independent, so a host may call [`register_all_with`] as well.
///
/// The [`Config`] goes into `lua.app_data` exactly as in
/// [`register_all_with`], with the same replace-on-repeat semantics.
///
/// The prefix is the host's choice.  [`PRELOAD_PREFIX`] matches the
/// module names the shipped Teal declarations use; a host that names its
/// namespace differently writes the declarations under that prefix
/// instead (`dts::write_to(dir, prefix)`), so the two stay aligned.
pub fn preload_all_with(lua: &Lua, prefix: &str, config: Config) -> LuaResult<()> {
    lua.set_app_data(config);

    let preload: LuaTable = lua
        .globals()
        .get::<LuaTable>("package")?
        .get::<LuaTable>("preload")?;

    // `task` is async-first and stays out of the synchronous namespace
    // `register_all` builds, but a preload entry costs nothing until it is
    // required, so a Teal host reaches it the same way as the others.
    #[cfg(feature = "task")]
    let task_entry = Some(("task", task::module as ModuleFactory));
    #[cfg(not(feature = "task"))]
    let task_entry: Option<(&'static str, ModuleFactory)> = None;

    let mut names: Vec<&'static str> = Vec::new();
    for (name, factory) in module_entries().into_iter().chain(task_entry) {
        names.push(name);
        // A preload loader receives (modname, extra); neither is needed.
        let loader = lua.create_function(move |lua, _: LuaMultiValue| factory(lua))?;
        preload.set(format!("{prefix}.{name}"), loader)?;
    }

    let prefix_owned = prefix.to_string();
    let namespace_loader = lua.create_function(move |lua, _: LuaMultiValue| {
        let require: LuaFunction = lua.globals().get("require")?;
        let ns = lua.create_table()?;
        for name in &names {
            let module: LuaTable = require.call(format!("{prefix_owned}.{name}"))?;
            ns.set(*name, module)?;
        }
        Ok(ns)
    })?;
    preload.set(prefix, namespace_loader)?;

    Ok(())
}

/// Returns a list of `(name, factory)` pairs for all enabled modules.
///
/// Each entry is a `(&'static str, fn(&Lua) -> LuaResult<LuaTable>)`.
/// The list only includes modules whose cargo features are active.
///
/// # When to use
///
/// Use this when you need per-module registration instead of the
/// all-in-one [`register_all`]. Common case: integration with
/// `mlua-pkg`'s `NativeResolver`:
///
/// ```rust,ignore
/// // `ignore`: NativeResolver is from the `mlua-pkg` crate, which is
/// // not a dependency of this crate. Cannot be compiled in-tree.
/// let mut resolver = NativeResolver::new();
/// for (name, factory) in mlua_batteries::module_entries() {
///     resolver = resolver.add(name, |lua| factory(lua).map(mlua::Value::Table));
/// }
/// ```
pub fn module_entries() -> Vec<(&'static str, ModuleFactory)> {
    let mut entries: Vec<(&'static str, ModuleFactory)> = Vec::new();

    macro_rules! entry {
        ($name:literal, $mod:ident) => {{
            #[cfg(feature = $name)]
            entries.push(($name, $mod::module));
        }};
    }

    entry!("json", json);
    entry!("env", env);
    entry!("path", path);
    entry!("string", string);
    entry!("regex", regex);
    entry!("validate", validate);
    entry!("pretty", pretty);
    entry!("log", log);
    entry!("uuid", uuid);
    entry!("base64", base64);
    entry!("time", time);
    entry!("fs", fs);
    entry!("http", http);
    entry!("llm", llm);
    entry!("hash", hash);
    entry!("proc", proc);
    entry!("watch", watch);

    entries
}
