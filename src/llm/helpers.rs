//! Shared helper functions for LLM providers.

use secrecy::{ExposeSecret, SecretString};
use std::time::Duration;

use super::types::*;

pub(super) fn effective_base_url<'a>(
    provider: &'a dyn LlmProvider,
    explicit: &'a Option<String>,
) -> &'a str {
    if let Some(url) = explicit {
        return url;
    }
    provider.default_base_url().unwrap_or_default()
}

/// Resolve the API key from the request or a fallback environment variable.
///
/// # Note on `EnvPolicy` bypass
///
/// This function reads the environment variable via [`std::env::var`]
/// directly, intentionally **bypassing** the [`EnvPolicy`](crate::policy::EnvPolicy).
/// Rationale: API keys are infrastructure credentials supplied by the host
/// application, not user-controlled data. Routing them through `EnvPolicy`
/// would require the Lua sandbox operator to explicitly allow sensitive
/// variable names (e.g. `OPENAI_API_KEY`) in the allow-list, which is
/// counter-intuitive — those variables should *not* be readable by Lua
/// scripts. The LLM module needs the key to authenticate, but never
/// exposes it to Lua land (it is wrapped in [`SecretString`]).
pub(super) fn resolve_api_key(
    request_key: &Option<SecretString>,
    env_var: &str,
) -> Result<SecretString, String> {
    if let Some(key) = request_key {
        if !key.expose_secret().is_empty() {
            return Ok(key.clone());
        }
    }
    match std::env::var(env_var) {
        Ok(key) if !key.is_empty() => Ok(SecretString::from(key)),
        _ => Err(format!("{env_var} not set and no api_key provided")),
    }
}

/// Merge `extra` fields into the request body.
///
/// # Warning
///
/// `extra` can override **any** field in the request body, including
/// `model` and `messages`.  If your application needs to restrict
/// which fields are overridable, validate `extra` before passing it
/// to the provider.
///
/// # Rationale
///
/// This intentionally allows overriding core fields.  The caller
/// already controls the API key and all request parameters — there
/// is no security boundary between the Lua script and the provider
/// request.  No major LLM vendor guards against client-side field
/// override either, so restricting it here would add complexity
/// without real benefit.
pub(super) fn merge_extra(body: &mut serde_json::Value, extra: &Option<serde_json::Value>) {
    if let (Some(serde_json::Value::Object(extra_map)), serde_json::Value::Object(body_map)) =
        (extra, body)
    {
        for (k, v) in extra_map {
            body_map.insert(k.clone(), v.clone());
        }
    }
}

/// Create an [`ureq::Agent`] with a per-request timeout, reusing the
/// provider's default agent when the timeout matches.
pub(super) fn agent_with_timeout(
    default: &ureq::Agent,
    default_secs: u64,
    request_secs: u64,
) -> ureq::Agent {
    if request_secs == default_secs {
        default.clone()
    } else {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(request_secs)))
            .build();
        ureq::Agent::new_with_config(config)
    }
}

/// Saturating conversion from u64 to u32.
pub(super) fn sat_u32(val: u64) -> u32 {
    val.min(u32::MAX as u64) as u32
}

/// Send a JSON POST request and parse the response as JSON.
///
/// Handles serialization, header injection, body reading with limit,
/// and deserialization — the common request/response path shared by
/// all LLM providers.
pub(super) fn post_json(
    agent: &ureq::Agent,
    url: &str,
    body: &serde_json::Value,
    headers: &[(&str, &str)],
    max_response_bytes: u64,
) -> Result<serde_json::Value, String> {
    let body_str = serde_json::to_string(body).map_err(|e| format!("json serialize: {e}"))?;
    let mut req = agent.post(url).content_type("application/json");
    for &(k, v) in headers {
        req = req.header(k, v);
    }
    let mut resp = req.send(body_str.as_bytes()).map_err(|e| e.to_string())?;
    let resp_body = resp
        .body_mut()
        .with_config()
        .limit(max_response_bytes)
        .read_to_string()
        .map_err(|e| format!("read body: {e}"))?;
    serde_json::from_str(&resp_body).map_err(|e| format!("json parse: {e}"))
}

/// Check for an API error in the response JSON.
///
/// Supports two common formats:
/// - OpenAI / Anthropic: `{"error": {"message": "..."}}`
/// - Ollama: `{"error": "..."}`
///
/// `provider` is used to prefix the error message so that callers can
/// identify which provider returned the error (e.g. `"openai: invalid key"`).
///
/// Returns `Err(message)` if an error field is present.
pub(super) fn check_api_error(json: &serde_json::Value, provider: &str) -> Result<(), String> {
    if let Some(err) = json.get("error") {
        let detail = err
            .get("message")
            .and_then(|m| m.as_str())
            .or_else(|| err.as_str())
            .unwrap_or("unknown error");
        return Err(format!("{provider}: {detail}"));
    }
    Ok(())
}

pub(super) fn parse_finish_reason_openai(s: &str) -> FinishReason {
    match s {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::MaxTokens,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Error,
    }
}
