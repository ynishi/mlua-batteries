//! Ollama native API provider.

use std::time::Duration;

use super::helpers::*;
use super::types::*;

/// Ollama native API provider (`/api/chat`).
///
/// No API key required. Images are supported via base64 only
/// (Ollama does not support image URLs).
pub struct OllamaProvider {
    agent: ureq::Agent,
    default_timeout_secs: u64,
}

impl OllamaProvider {
    const DEFAULT_BASE_URL: &str = "http://localhost:11434";

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

impl LlmProvider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    fn default_base_url(&self) -> Option<&str> {
        Some(Self::DEFAULT_BASE_URL)
    }

    fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, String> {
        let base_url = request
            .base_url
            .as_deref()
            .unwrap_or(Self::DEFAULT_BASE_URL);
        let url = format!("{base_url}/api/chat");

        let mut messages = Vec::new();
        if let Some(sys) = &request.system {
            messages.push(serde_json::json!({"role": "system", "content": sys}));
        }
        for msg in &request.messages {
            messages.push(serialize_message_ollama(msg)?);
        }

        let mut body = serde_json::json!({
            "model": request.model,
            "messages": messages,
            "stream": false,
        });

        let mut options = serde_json::Map::new();
        if let Some(temp) = request.temperature {
            options.insert("temperature".into(), serde_json::json!(temp));
        }
        if let Some(max) = request.max_tokens {
            options.insert("num_predict".into(), serde_json::json!(max));
        }
        if let Some(top_p) = request.top_p {
            options.insert("top_p".into(), serde_json::json!(top_p));
        }
        if let Some(stop) = &request.stop {
            options.insert("stop".into(), serde_json::json!(stop));
        }
        // Merge extra into options (Ollama uses options object for params)
        if let Some(serde_json::Value::Object(extra_map)) = &request.extra {
            for (k, v) in extra_map {
                options.insert(k.clone(), v.clone());
            }
        }
        if !options.is_empty() {
            body["options"] = serde_json::Value::Object(options);
        }

        let agent =
            agent_with_timeout(&self.agent, self.default_timeout_secs, request.timeout_secs);
        let json = post_json(&agent, &url, &body, &[], request.max_response_bytes)?;
        check_api_error(&json, "ollama")?;

        let content = json
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string();

        let finish_reason = match json.get("done_reason").and_then(|r| r.as_str()) {
            Some("stop") => FinishReason::Stop,
            Some("length") => FinishReason::MaxTokens,
            _ => FinishReason::Stop,
        };

        let usage = Usage {
            input_tokens: sat_u32(
                json.get("prompt_eval_count")
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0),
            ),
            output_tokens: sat_u32(json.get("eval_count").and_then(|n| n.as_u64()).unwrap_or(0)),
        };

        Ok(ChatResponse {
            content,
            finish_reason,
            usage,
            model: request.model.clone(),
        })
    }
}

pub(super) fn serialize_message_ollama(msg: &ChatMessage) -> Result<serde_json::Value, String> {
    let role = match msg.role {
        ChatRole::User => "user",
        ChatRole::Assistant => "assistant",
    };
    match &msg.content {
        ChatContent::Text(s) => Ok(serde_json::json!({"role": role, "content": s})),
        ChatContent::Parts(parts) => {
            let text: String = parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");

            let images: Vec<&str> = parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::ImageBase64 { data, .. } => Some(data.as_str()),
                    _ => None,
                })
                .collect();

            // Ollama does not support image URLs
            let has_url = parts
                .iter()
                .any(|p| matches!(p, ContentPart::ImageUrl { .. }));
            if has_url {
                return Err("Ollama does not support image URLs; use image_base64 instead".into());
            }

            let mut msg_json = serde_json::json!({"role": role, "content": text});
            if !images.is_empty() {
                msg_json["images"] = serde_json::json!(images);
            }
            Ok(msg_json)
        }
    }
}
