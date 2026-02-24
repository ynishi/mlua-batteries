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
pub mod policy;

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
#[cfg(feature = "path")]
pub mod path;
#[cfg(feature = "time")]
pub mod time;

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
    register!("time", time);
    register!("fs", fs);
    register!("http", http);
    register!("llm", llm);
    register!("hash", hash);

    lua.globals().set(namespace, ns.clone())?;
    Ok(ns)
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
    entry!("time", time);
    entry!("fs", fs);
    entry!("http", http);
    entry!("llm", llm);
    entry!("hash", hash);

    entries
}
