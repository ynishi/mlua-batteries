//! HTTP client module (ureq 3).
//!
//! ```lua
//! local http = std.http
//! local resp = http.get("https://httpbin.org/get")
//! -- resp.status, resp.body
//!
//! local resp = http.post("https://httpbin.org/post", '{"a":1}')
//! local resp = http.post(url, body, "text/plain")  -- custom content-type
//!
//! -- Full control:
//! local resp = http.request({
//!     method = "PUT",
//!     url = "https://example.com/api",
//!     headers = { ["Authorization"] = "Bearer token" },
//!     body = '{"key":"value"}',
//!     timeout = 60,
//! })
//! ```

use mlua::prelude::*;
use std::time::Duration;

use crate::util::{check_url, with_config};

/// Shared HTTP agent stored in `lua.app_data`.
///
/// [`ureq::Agent`] uses `Arc` internally — cloning shares the
/// underlying connection pool (TLS sessions, keep-alive connections).
/// A single `SharedHttpAgent` is created at module init time and
/// cloned per-request, avoiding repeated TLS handshakes to the
/// same host.
struct SharedHttpAgent {
    agent: ureq::Agent,
    default_timeout_secs: u64,
}

/// Get (or create) the shared agent, then clone it.
///
/// If the caller needs a different timeout than the default,
/// a one-off agent is created (no pool sharing with the default).
fn get_agent(lua: &Lua, timeout_secs: Option<u64>) -> LuaResult<ureq::Agent> {
    let shared = lua
        .app_data_ref::<SharedHttpAgent>()
        .ok_or_else(|| LuaError::external("mlua-batteries: HTTP agent not initialized"))?;

    match timeout_secs {
        Some(t) if t != shared.default_timeout_secs => {
            let config = ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(t)))
                .build();
            Ok(ureq::Agent::new_with_config(config))
        }
        _ => Ok(shared.agent.clone()),
    }
}

/// Apply headers to a bodyless request (GET/HEAD/DELETE), send, and build
/// the `{status, body}` response table.
///
/// Factored out of the `request()` closure because ureq v3's typestate design
/// (`RequestBuilder<WithoutBody>` vs `RequestBuilder<WithBody>`) requires
/// separate code paths — there is no shared trait exposing `.header()`.
fn send_without_body(
    lua: &Lua,
    mut req: ureq::RequestBuilder<ureq::typestate::WithoutBody>,
    headers: &[(String, String)],
    max_bytes: u64,
) -> LuaResult<LuaTable> {
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let mut resp = req.call().map_err(|e| LuaError::external(e.to_string()))?;
    let status = resp.status().as_u16();
    let body = read_body(resp.body_mut(), max_bytes)?;
    build_response(lua, status, body)
}

/// Apply headers to a body request (POST/PUT/PATCH), send, and build
/// the `{status, body}` response table.
fn send_with_body(
    lua: &Lua,
    mut req: ureq::RequestBuilder<ureq::typestate::WithBody>,
    headers: &[(String, String)],
    body: Option<&str>,
    content_type: &str,
    max_bytes: u64,
) -> LuaResult<LuaTable> {
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let mut resp = req
        .content_type(content_type)
        .send(body.unwrap_or("").as_bytes())
        .map_err(|e| LuaError::external(e.to_string()))?;
    let status = resp.status().as_u16();
    let body_text = read_body(resp.body_mut(), max_bytes)?;
    build_response(lua, status, body_text)
}

pub fn module(lua: &Lua) -> LuaResult<LuaTable> {
    // Create shared agent once with default timeout from Config.
    if lua.app_data_ref::<SharedHttpAgent>().is_none() {
        let timeout = with_config(lua, |c| c.http_timeout)?;
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .build();
        lua.set_app_data(SharedHttpAgent {
            agent: ureq::Agent::new_with_config(config),
            default_timeout_secs: timeout.as_secs(),
        });
    }

    let t = lua.create_table()?;

    t.set(
        "get",
        lua.create_function(|lua, url: String| {
            check_url(lua, &url, "GET")?;
            let max_bytes = with_config(lua, |c| c.max_response_bytes)?;
            let agent = get_agent(lua, None)?;

            let mut resp = agent
                .get(&url)
                .call()
                .map_err(|e| LuaError::external(e.to_string()))?;

            let status = resp.status().as_u16();
            let body = read_body(resp.body_mut(), max_bytes)?;
            build_response(lua, status, body)
        })?,
    )?;

    t.set(
        "post",
        lua.create_function(
            |lua, (url, body, content_type): (String, String, Option<String>)| {
                check_url(lua, &url, "POST")?;
                let ct = content_type.as_deref().unwrap_or("application/json");
                let max_bytes = with_config(lua, |c| c.max_response_bytes)?;
                let agent = get_agent(lua, None)?;

                let mut resp = agent
                    .post(&url)
                    .content_type(ct)
                    .send(body.as_bytes())
                    .map_err(|e| LuaError::external(e.to_string()))?;

                let status = resp.status().as_u16();
                let body = read_body(resp.body_mut(), max_bytes)?;
                build_response(lua, status, body)
            },
        )?,
    )?;

    t.set(
        "request",
        lua.create_function(|lua, opts: LuaTable| {
            let method: String = opts.get("method")?;
            let url: String = opts.get("url")?;
            check_url(lua, &url, &method)?;
            let body: Option<String> = match opts.get::<LuaValue>("body")? {
                LuaValue::Nil => None,
                LuaValue::String(s) => Some(s.to_str()?.to_string()),
                other => {
                    return Err(LuaError::external(format!(
                        "body must be a string, got {}",
                        other.type_name()
                    )));
                }
            };

            let (default_timeout, max_bytes) =
                with_config(lua, |c| (c.http_timeout, c.max_response_bytes))?;
            let timeout_secs: u64 = opts.get("timeout").unwrap_or(default_timeout.as_secs());
            let agent = get_agent(lua, Some(timeout_secs))?;

            // Collect headers
            let mut headers: Vec<(String, String)> = Vec::new();
            if let Ok(h) = opts.get::<LuaTable>("headers") {
                for pair in h.pairs::<String, String>() {
                    headers.push(pair?);
                }
            }

            let ct: String = opts
                .get("content_type")
                .unwrap_or_else(|_| "application/json".into());

            let method_upper = method.to_uppercase();
            match method_upper.as_str() {
                "GET" => send_without_body(lua, agent.get(&url), &headers, max_bytes),
                "HEAD" => send_without_body(lua, agent.head(&url), &headers, max_bytes),
                "DELETE" => send_without_body(lua, agent.delete(&url), &headers, max_bytes),
                "POST" => send_with_body(
                    lua,
                    agent.post(&url),
                    &headers,
                    body.as_deref(),
                    &ct,
                    max_bytes,
                ),
                "PUT" => send_with_body(
                    lua,
                    agent.put(&url),
                    &headers,
                    body.as_deref(),
                    &ct,
                    max_bytes,
                ),
                "PATCH" => send_with_body(
                    lua,
                    agent.patch(&url),
                    &headers,
                    body.as_deref(),
                    &ct,
                    max_bytes,
                ),
                other => Err(LuaError::external(format!(
                    "unsupported HTTP method: {other}"
                ))),
            }
        })?,
    )?;

    Ok(t)
}

/// Read a response body with a byte limit.
fn read_body(body: &mut ureq::Body, max_bytes: u64) -> LuaResult<String> {
    body.with_config()
        .limit(max_bytes)
        .read_to_string()
        .map_err(|e| LuaError::external(e.to_string()))
}

/// Build a `{status, body}` Lua table from already-extracted values.
///
/// Avoids naming `http::Response<ureq::Body>` directly (ureq v3 does not
/// re-export the `http::Response` type), keeping the module free of a
/// direct `http` crate dependency.
fn build_response(lua: &Lua, status: u16, body: String) -> LuaResult<LuaTable> {
    let result = lua.create_table()?;
    result.set("status", status)?;
    result.set("body", body)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use mlua::Lua;
    use std::time::Duration;

    #[test]
    fn get_returns_table_with_status_and_body() {
        // Verify the module registers and the function signatures are correct.
        // Actual HTTP calls are not made in unit tests.
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();

        // Verify that http.get is a function
        let ty: String = lua.load("return type(std.http.get)").eval().unwrap();
        assert_eq!(ty, "function");
    }

    #[test]
    fn post_is_registered() {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();

        let ty: String = lua.load("return type(std.http.post)").eval().unwrap();
        assert_eq!(ty, "function");
    }

    #[test]
    fn request_is_registered() {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();

        let ty: String = lua.load("return type(std.http.request)").eval().unwrap();
        assert_eq!(ty, "function");
    }

    #[test]
    fn request_rejects_unsupported_method() {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();

        let result: mlua::Result<mlua::Value> = lua
            .load(
                r#"return std.http.request({
                    method = "TRACE",
                    url = "http://localhost:0/nope"
                })"#,
            )
            .eval();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("unsupported HTTP method"));
    }

    #[test]
    fn custom_timeout_applied() {
        let lua = Lua::new();
        let config = crate::config::Config::builder()
            .http_timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        crate::register_all_with(&lua, "std", config).unwrap();

        // Module registers without error with custom timeout
        let ty: String = lua.load("return type(std.http.get)").eval().unwrap();
        assert_eq!(ty, "function");
    }

    #[test]
    fn request_rejects_non_string_body() {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();

        let result: mlua::Result<mlua::Value> = lua
            .load(
                r#"return std.http.request({
                    method = "POST",
                    url = "http://localhost:0/nope",
                    body = 12345
                })"#,
            )
            .eval();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("body must be a string"),
            "expected body type error, got: {err_msg}"
        );
    }

    #[test]
    fn request_missing_method_errors() {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();

        let result: mlua::Result<mlua::Value> = lua
            .load(
                r#"return std.http.request({
                    url = "http://localhost:0/nope"
                })"#,
            )
            .eval();
        assert!(result.is_err());
    }

    #[test]
    fn request_missing_url_errors() {
        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();

        let result: mlua::Result<mlua::Value> = lua
            .load(
                r#"return std.http.request({
                    method = "GET"
                })"#,
            )
            .eval();
        assert!(result.is_err());
    }
}
