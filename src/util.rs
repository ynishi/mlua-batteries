use mlua::prelude::*;

use crate::config::Config;
use crate::policy::{FsAccess, PathOp};

/// Resolve a path through the active [`PathPolicy`](crate::policy::PathPolicy).
///
/// Returns an [`FsAccess`] handle — all subsequent I/O MUST go through
/// this handle's methods, never via `std::fs` directly.
pub(crate) fn check_path(lua: &Lua, path: &str, op: PathOp) -> LuaResult<FsAccess> {
    let config = lua.app_data_ref::<Config>().ok_or_else(|| {
        LuaError::external(
            "mlua-batteries: Config not initialized (use register_all or register_all_with)",
        )
    })?;
    config
        .path_policy
        .resolve(std::path::Path::new(path), op)
        .map_err(LuaError::external)
}

/// Check a URL through the active [`HttpPolicy`](crate::policy::HttpPolicy).
#[cfg(feature = "http")]
pub(crate) fn check_url(lua: &Lua, url: &str, method: &str) -> LuaResult<()> {
    let config = lua
        .app_data_ref::<Config>()
        .ok_or_else(|| LuaError::external("mlua-batteries: Config not initialized"))?;
    config
        .http_policy
        .check_url(url, method)
        .map_err(LuaError::external)
}

/// Check an env var read through the active [`EnvPolicy`](crate::policy::EnvPolicy).
pub(crate) fn check_env_get(lua: &Lua, key: &str) -> LuaResult<()> {
    let config = lua
        .app_data_ref::<Config>()
        .ok_or_else(|| LuaError::external("mlua-batteries: Config not initialized"))?;
    config.env_policy.check_get(key).map_err(LuaError::external)
}

/// Check an env var set through the active [`EnvPolicy`](crate::policy::EnvPolicy).
pub(crate) fn check_env_set(lua: &Lua, key: &str) -> LuaResult<()> {
    let config = lua
        .app_data_ref::<Config>()
        .ok_or_else(|| LuaError::external("mlua-batteries: Config not initialized"))?;
    config.env_policy.check_set(key).map_err(LuaError::external)
}

/// Check an LLM request through the active [`LlmPolicy`](crate::policy::LlmPolicy).
#[cfg(feature = "llm")]
pub(crate) fn check_llm_request(
    lua: &Lua,
    provider: &str,
    model: &str,
    base_url: &str,
) -> LuaResult<()> {
    let config = lua
        .app_data_ref::<Config>()
        .ok_or_else(|| LuaError::external("mlua-batteries: Config not initialized"))?;
    config
        .llm_policy
        .check_request(provider, model, base_url)
        .map_err(LuaError::external)
}

/// Read a config value via a closure.
///
/// Avoids repeating the "config not initialized" boilerplate.
pub(crate) fn with_config<T>(lua: &Lua, f: impl FnOnce(&Config) -> T) -> LuaResult<T> {
    let config = lua
        .app_data_ref::<Config>()
        .ok_or_else(|| LuaError::external("mlua-batteries: Config not initialized"))?;
    Ok(f(&config))
}

/// Classification of a Lua table as either a contiguous integer-keyed
/// array or a general key-value map.
pub(crate) enum TableKind {
    /// Contiguous integer keys `1..=len` with no extra keys.
    Array(usize),
    /// General key-value pairs (may include integer keys).
    Map(Vec<(LuaValue, LuaValue)>),
}

/// Inspect a Lua table and classify it as array or map.
///
/// A table is considered an array when:
/// - `raw_len() > 0`
/// - every key is an integer in `1..=raw_len()`
/// - the number of pairs equals `raw_len()`
///
/// **Consequence:** an empty table (`raw_len() == 0`) is always
/// classified as `Map` with zero pairs. This means `json.encode({})`
/// produces `{}` (object), not `[]` (array).
///
/// # Performance
///
/// Collects all pairs into a `Vec` before inspecting keys.  This
/// performs a full table traversal followed by a linear scan — two
/// passes over the data.  An alternative (early-exit iterator) would
/// save the allocation, but the `Map` variant needs the collected
/// pairs anyway, so the Vec is reused in the common case.
///
/// JSON serialization (the primary caller) typically operates on
/// small-to-medium tables where this cost is negligible.
pub(crate) fn classify(table: &LuaTable) -> LuaResult<TableKind> {
    let len = table.raw_len();
    let pairs: Vec<(LuaValue, LuaValue)> = table
        .pairs::<LuaValue, LuaValue>()
        .collect::<LuaResult<Vec<_>>>()?;

    let len_i64 = i64::try_from(len).unwrap_or(i64::MAX);
    let is_array = len > 0
        && pairs.len() == len
        && pairs
            .iter()
            .all(|(k, _)| matches!(k, LuaValue::Integer(i) if *i >= 1 && *i <= len_i64));

    if is_array {
        Ok(TableKind::Array(len))
    } else {
        Ok(TableKind::Map(pairs))
    }
}

/// Create a Lua instance with all modules registered under "std",
/// then evaluate `code` and return the result.
///
/// Panics on Lua errors — intended for tests only.
#[cfg(test)]
pub(crate) fn test_eval<T: mlua::FromLua>(code: &str) -> T {
    let lua = Lua::new();
    crate::register_all(&lua, "std").unwrap();
    lua.load(code).eval().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_table_is_map() {
        let lua = Lua::new();
        let t: LuaTable = lua.load("return {}").eval().unwrap();
        assert!(matches!(classify(&t).unwrap(), TableKind::Map(pairs) if pairs.is_empty()));
    }

    #[test]
    fn contiguous_array() {
        let lua = Lua::new();
        let t: LuaTable = lua.load("return {10, 20, 30}").eval().unwrap();
        assert!(matches!(classify(&t).unwrap(), TableKind::Array(3)));
    }

    #[test]
    fn string_keyed_is_map() {
        let lua = Lua::new();
        let t: LuaTable = lua.load("return {a=1, b=2}").eval().unwrap();
        assert!(matches!(classify(&t).unwrap(), TableKind::Map(pairs) if pairs.len() == 2));
    }

    #[test]
    fn mixed_keys_is_map() {
        let lua = Lua::new();
        let t: LuaTable = lua.load("return {10, 20, a='x'}").eval().unwrap();
        assert!(matches!(classify(&t).unwrap(), TableKind::Map(pairs) if pairs.len() == 3));
    }

    #[test]
    fn sparse_int_keys_is_map() {
        let lua = Lua::new();
        let t: LuaTable = lua
            .load("local t = {}; t[1] = 'a'; t[3] = 'c'; return t")
            .eval()
            .unwrap();
        assert!(matches!(classify(&t).unwrap(), TableKind::Map(_)));
    }

    #[test]
    fn single_element_array() {
        let lua = Lua::new();
        let t: LuaTable = lua.load("return {'only'}").eval().unwrap();
        assert!(matches!(classify(&t).unwrap(), TableKind::Array(1)));
    }
}
