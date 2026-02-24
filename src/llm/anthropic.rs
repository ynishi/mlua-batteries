//! Anthropic Messages API provider.

use secrecy::ExposeSecret;
use std::time::Duration;

use super::helpers::*;
use super::types::*;

/// Anthropic Messages API provider.
///
/// API key: `api_key` field in request, or `ANTHROPIC_API_KEY` env var.
///
/// Uses `anthropic-version: 2023-06-01` (the current stable API version
/// as of the Messages API launch; Anthropic SDKs also hardcode this).
pub struct AnthropicProvider {
    agent: ureq::Agent,
    default_timeout_secs: u64,
}

impl AnthropicProvider {
    const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

    /// Create a new provider with a dedicated `ureq::Agent`.
    pub fn new(default_timeout_secs: u64) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(default_timeout_secs)))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
            default_timeout_secs,
        }
    }

    /// Create a new provider sharing the given `ureq::Agent`.
    ///
    /// The shared agent's connection pool (TLS sessions, keep-alive)
    /// is reused across providers, reducing handshake overhead.
    pub fn with_agent(agent: ureq::Agent, default_timeout_secs: u64) -> Self {
        Self {
            agent,
            default_timeout_secs,
        }
    }
}

impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn default_base_url(&self) -> Option<&str> {
        Some(Self::DEFAULT_BASE_URL)
    }

    fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, String> {
        let api_key = resolve_api_key(&request.api_key, "ANTHROPIC_API_KEY")?;
        let base_url = request
            .base_url
            .as_deref()
            .unwrap_or(Self::DEFAULT_BASE_URL);
        let url = format!("{base_url}/v1/messages");

        let mut messages = Vec::new();
        for msg in &request.messages {
            messages.push(serialize_message_anthropic(msg)?);
        }

        let mut body = serde_json::json!({
            "model": request.model,
            "messages": messages,
            "max_tokens": request.max_tokens.unwrap_or(1024),
        });
        if let Some(sys) = &request.system {
            body["system"] = serde_json::json!(sys);
        }
        if let Some(temp) = request.temperature {
            body["temperature"] = serde_json::json!(temp);
        }
        if let Some(top_p) = request.top_p {
            body["top_p"] = serde_json::json!(top_p);
        }
        if let Some(stop) = &request.stop {
            body["stop_sequences"] = serde_json::json!(stop);
        }
        merge_extra(&mut body, &request.extra);

        let agent =
            agent_with_timeout(&self.agent, self.default_timeout_secs, request.timeout_secs);
        let json = post_json(
            &agent,
            &url,
            &body,
            &[
                ("x-api-key", api_key.expose_secret()),
                ("anthropic-version", "2023-06-01"),
            ],
            request.max_response_bytes,
        )?;
        check_api_error(&json, "anthropic")?;

        // Anthropic responses may contain multiple content blocks
        // (e.g. text interspersed with tool_use). We extract all text
        // blocks and join them with "\n".
        let content = json
            .get("content")
            .and_then(|c| c.as_array())
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|b| {
                        if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                            b.get("text").and_then(|t| t.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();

        let finish_reason = match json.get("stop_reason").and_then(|r| r.as_str()) {
            Some("end_turn") => FinishReason::Stop,
            Some("max_tokens") => FinishReason::MaxTokens,
            _ => FinishReason::Stop,
        };

        let usage = json
            .get("usage")
            .map(|u| Usage {
                input_tokens: sat_u32(u.get("input_tokens").and_then(|n| n.as_u64()).unwrap_or(0)),
                output_tokens: sat_u32(
                    u.get("output_tokens").and_then(|n| n.as_u64()).unwrap_or(0),
                ),
            })
            .unwrap_or_default();

        let model = json
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or(&request.model)
            .to_string();

        Ok(ChatResponse {
            content,
            finish_reason,
            usage,
            model,
        })
    }
}

fn serialize_message_anthropic(msg: &ChatMessage) -> Result<serde_json::Value, String> {
    let role = match msg.role {
        ChatRole::User => "user",
        ChatRole::Assistant => "assistant",
    };
    match &msg.content {
        ChatContent::Text(s) => Ok(serde_json::json!({"role": role, "content": s})),
        ChatContent::Parts(parts) => {
            let arr: Result<Vec<serde_json::Value>, String> = parts
                .iter()
                .map(|p| match p {
                    ContentPart::Text { text } => {
                        Ok(serde_json::json!({"type": "text", "text": text}))
                    }
                    ContentPart::ImageUrl { url } => Ok(serde_json::json!({
                        "type": "image",
                        "source": {"type": "url", "url": url}
                    })),
                    ContentPart::ImageBase64 { data, media_type } => Ok(serde_json::json!({
                        "type": "image",
                        "source": {"type": "base64", "media_type": media_type, "data": data}
                    })),
                })
                .collect();
            Ok(serde_json::json!({"role": role, "content": arr?}))
        }
    }
}
