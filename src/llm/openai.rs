//! OpenAI-compatible chat completion provider.

use secrecy::ExposeSecret;
use std::time::Duration;

use super::helpers::*;
use super::types::*;

/// OpenAI-compatible chat completion provider.
///
/// Works with the OpenAI API and any OpenAI-compatible endpoint
/// (e.g. vLLM, LiteLLM proxy).
///
/// API key: `api_key` field in request, or `OPENAI_API_KEY` env var.
pub struct OpenAiProvider {
    agent: ureq::Agent,
    default_timeout_secs: u64,
}

impl OpenAiProvider {
    const DEFAULT_BASE_URL: &str = "https://api.openai.com";

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

impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn default_base_url(&self) -> Option<&str> {
        Some(Self::DEFAULT_BASE_URL)
    }

    fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, String> {
        let api_key = resolve_api_key(&request.api_key, "OPENAI_API_KEY")?;
        let base_url = request
            .base_url
            .as_deref()
            .unwrap_or(Self::DEFAULT_BASE_URL);
        let url = format!("{base_url}/v1/chat/completions");

        let mut messages = Vec::new();
        if let Some(sys) = &request.system {
            messages.push(serde_json::json!({"role": "system", "content": sys}));
        }
        for msg in &request.messages {
            messages.push(serialize_message_openai(msg)?);
        }

        let mut body = serde_json::json!({
            "model": request.model,
            "messages": messages,
        });
        if let Some(max) = request.max_tokens {
            body["max_tokens"] = serde_json::json!(max);
        }
        if let Some(temp) = request.temperature {
            body["temperature"] = serde_json::json!(temp);
        }
        if let Some(top_p) = request.top_p {
            body["top_p"] = serde_json::json!(top_p);
        }
        if let Some(stop) = &request.stop {
            body["stop"] = serde_json::json!(stop);
        }
        merge_extra(&mut body, &request.extra);

        let auth = format!("Bearer {}", api_key.expose_secret());

        let agent =
            agent_with_timeout(&self.agent, self.default_timeout_secs, request.timeout_secs);
        let json = post_json(
            &agent,
            &url,
            &body,
            &[("Authorization", &auth)],
            request.max_response_bytes,
        )?;
        check_api_error(&json, "openai")?;

        let choice = json
            .get("choices")
            .and_then(|c| c.get(0))
            .ok_or("no choices in response")?;

        let content = choice
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string();

        let finish_reason = choice
            .get("finish_reason")
            .and_then(|r| r.as_str())
            .map(parse_finish_reason_openai)
            .unwrap_or(FinishReason::Stop);

        let usage = json
            .get("usage")
            .map(|u| Usage {
                input_tokens: sat_u32(u.get("prompt_tokens").and_then(|n| n.as_u64()).unwrap_or(0)),
                output_tokens: sat_u32(
                    u.get("completion_tokens")
                        .and_then(|n| n.as_u64())
                        .unwrap_or(0),
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

fn serialize_message_openai(msg: &ChatMessage) -> Result<serde_json::Value, String> {
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
                    ContentPart::ImageUrl { url } => {
                        Ok(serde_json::json!({"type": "image_url", "image_url": {"url": url}}))
                    }
                    ContentPart::ImageBase64 { data, media_type } => {
                        let data_url = format!("data:{media_type};base64,{data}");
                        Ok(serde_json::json!({"type": "image_url", "image_url": {"url": data_url}}))
                    }
                })
                .collect();
            Ok(serde_json::json!({"role": role, "content": arr?}))
        }
    }
}
