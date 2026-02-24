use std::time::Duration;

use secrecy::SecretString;

use super::helpers::*;
use super::ollama::serialize_message_ollama;
use super::*;

#[test]
fn module_registers_chat_and_batch() {
    let lua = mlua::Lua::new();
    crate::register_all(&lua, "std").unwrap();

    let ty: String = lua.load("return type(std.llm.chat)").eval().unwrap();
    assert_eq!(ty, "function");

    let ty: String = lua.load("return type(std.llm.batch)").eval().unwrap();
    assert_eq!(ty, "function");
}

#[test]
fn chat_requires_provider() {
    let lua = mlua::Lua::new();
    crate::register_all(&lua, "std").unwrap();

    let result: mlua::Result<mlua::Value> = lua
        .load(r#"return std.llm.chat({ model = "x", prompt = "hi" })"#)
        .eval();
    assert!(result.is_err());
}

#[test]
fn chat_requires_model() {
    let lua = mlua::Lua::new();
    crate::register_all(&lua, "std").unwrap();

    let result: mlua::Result<mlua::Value> = lua
        .load(r#"return std.llm.chat({ provider = "openai", prompt = "hi" })"#)
        .eval();
    assert!(result.is_err());
}

#[test]
fn chat_requires_prompt_or_messages() {
    let lua = mlua::Lua::new();
    crate::register_all(&lua, "std").unwrap();

    let result: mlua::Result<mlua::Value> = lua
        .load(r#"return std.llm.chat({ provider = "openai", model = "gpt-4o" })"#)
        .eval();
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("prompt") || err.contains("messages"),
        "error should mention prompt/messages, got: {err}"
    );
}

#[test]
fn chat_rejects_unknown_provider() {
    let lua = mlua::Lua::new();
    crate::register_all(&lua, "std").unwrap();

    let result: mlua::Result<mlua::Value> = lua
        .load(
            r#"return std.llm.chat({
                provider = "nonexistent",
                model = "x",
                prompt = "hi",
            })"#,
        )
        .eval();
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("unknown LLM provider"), "got: {err}");
}

#[test]
fn chat_rejects_system_role_in_messages() {
    let lua = mlua::Lua::new();
    crate::register_all(&lua, "std").unwrap();

    let result: mlua::Result<mlua::Value> = lua
        .load(
            r#"return std.llm.chat({
                provider = "ollama",
                model = "llama3.2",
                messages = {
                    { role = "system", content = "bad" },
                    { role = "user", content = "hi" },
                },
            })"#,
        )
        .eval();
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("unsupported message role"), "got: {err}");
}

#[test]
fn llm_policy_blocks_provider() {
    use crate::policy::LlmAllowList;

    let lua = mlua::Lua::new();
    let config = crate::config::Config::builder()
        .llm_policy(LlmAllowList::new(["ollama"]))
        .build()
        .unwrap();
    crate::register_all_with(&lua, "std", config).unwrap();

    let result: mlua::Result<mlua::Value> = lua
        .load(
            r#"return std.llm.chat({
                provider = "openai",
                model = "gpt-4o",
                prompt = "hi",
            })"#,
        )
        .eval();
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("not in the allow list"),
        "should be policy denial, got: {err}"
    );
}

#[test]
fn batch_empty_returns_empty_table() {
    let lua = mlua::Lua::new();
    crate::register_all(&lua, "std").unwrap();

    let len: i64 = lua
        .load(
            r#"
            local results = std.llm.batch({})
            return #results
        "#,
        )
        .eval()
        .unwrap();
    assert_eq!(len, 0);
}

#[test]
fn register_custom_provider() {
    struct EchoProvider;
    impl LlmProvider for EchoProvider {
        fn name(&self) -> &str {
            "echo"
        }
        fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, String> {
            let text = match &request.messages.first() {
                Some(msg) => match &msg.content {
                    ChatContent::Text(s) => s.clone(),
                    ChatContent::Parts(_) => "parts".into(),
                },
                None => "empty".into(),
            };
            Ok(ChatResponse {
                content: format!("echo: {text}"),
                finish_reason: FinishReason::Stop,
                usage: Usage::default(),
                model: request.model.clone(),
            })
        }
    }

    let lua = mlua::Lua::new();
    crate::register_all(&lua, "std").unwrap();
    register_provider(&lua, EchoProvider).unwrap();

    let content: String = lua
        .load(
            r#"
            local resp = std.llm.chat({
                provider = "echo",
                model = "test",
                prompt = "hello world",
            })
            return resp.content
        "#,
        )
        .eval()
        .unwrap();
    assert_eq!(content, "echo: hello world");
}

#[test]
fn register_custom_provider_with_usage() {
    struct UsageProvider;
    impl LlmProvider for UsageProvider {
        fn name(&self) -> &str {
            "usage-test"
        }
        fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, String> {
            Ok(ChatResponse {
                content: "ok".into(),
                finish_reason: FinishReason::Stop,
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
                model: request.model.clone(),
            })
        }
    }

    let lua = mlua::Lua::new();
    crate::register_all(&lua, "std").unwrap();
    register_provider(&lua, UsageProvider).unwrap();

    let (input, output): (u32, u32) = lua
        .load(
            r#"
            local resp = std.llm.chat({
                provider = "usage-test",
                model = "m",
                prompt = "hi",
            })
            return resp.usage.input_tokens, resp.usage.output_tokens
        "#,
        )
        .eval()
        .unwrap();
    assert_eq!(input, 10);
    assert_eq!(output, 5);
}

#[test]
fn batch_with_custom_provider() {
    struct CountProvider;
    impl LlmProvider for CountProvider {
        fn name(&self) -> &str {
            "count"
        }
        fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, String> {
            Ok(ChatResponse {
                content: format!("model={}", request.model),
                finish_reason: FinishReason::Stop,
                usage: Usage::default(),
                model: request.model.clone(),
            })
        }
    }

    let lua = mlua::Lua::new();
    crate::register_all(&lua, "std").unwrap();
    register_provider(&lua, CountProvider).unwrap();

    let result: String = lua
        .load(
            r#"
            local results = std.llm.batch({
                { provider = "count", model = "a", prompt = "x" },
                { provider = "count", model = "b", prompt = "y" },
            })
            return results[1].content .. "|" .. results[2].content
        "#,
        )
        .eval()
        .unwrap();
    assert_eq!(result, "model=a|model=b");
}

#[test]
fn batch_error_entry_has_error_field() {
    struct FailProvider;
    impl LlmProvider for FailProvider {
        fn name(&self) -> &str {
            "fail"
        }
        fn chat(&self, _request: &ChatRequest) -> Result<ChatResponse, String> {
            Err("deliberate failure".into())
        }
    }

    let lua = mlua::Lua::new();
    crate::register_all(&lua, "std").unwrap();
    register_provider(&lua, FailProvider).unwrap();

    let err_msg: String = lua
        .load(
            r#"
            local results = std.llm.batch({
                { provider = "fail", model = "x", prompt = "hi" },
            })
            return results[1].error
        "#,
        )
        .eval()
        .unwrap();
    assert_eq!(err_msg, "deliberate failure");
}

#[test]
fn finish_reason_mapping() {
    use super::parse::finish_reason_str;
    assert_eq!(finish_reason_str(&FinishReason::Stop), "stop");
    assert_eq!(finish_reason_str(&FinishReason::MaxTokens), "max_tokens");
    assert_eq!(
        finish_reason_str(&FinishReason::ContentFilter),
        "content_filter"
    );
    assert_eq!(finish_reason_str(&FinishReason::Error), "error");
}

#[test]
fn provider_default_base_urls() {
    let openai = OpenAiProvider::new(120);
    assert_eq!(
        LlmProvider::default_base_url(&openai),
        Some("https://api.openai.com")
    );

    let anthropic = AnthropicProvider::new(120);
    assert_eq!(
        LlmProvider::default_base_url(&anthropic),
        Some("https://api.anthropic.com")
    );

    let ollama = OllamaProvider::new(120);
    assert_eq!(
        LlmProvider::default_base_url(&ollama),
        Some("http://localhost:11434")
    );
}

#[test]
fn effective_base_url_uses_explicit_override() {
    let provider = OpenAiProvider::new(120);
    let explicit = Some("http://custom:8080".into());
    assert_eq!(
        effective_base_url(&provider, &explicit),
        "http://custom:8080"
    );
}

#[test]
fn effective_base_url_falls_back_to_provider_default() {
    let provider = OpenAiProvider::new(120);
    assert_eq!(
        effective_base_url(&provider, &None),
        "https://api.openai.com"
    );
}

#[test]
fn effective_base_url_empty_when_no_default() {
    struct NoUrlProvider;
    impl LlmProvider for NoUrlProvider {
        fn name(&self) -> &str {
            "no-url"
        }
        fn chat(&self, _: &ChatRequest) -> Result<ChatResponse, String> {
            unreachable!("NoUrlProvider should not be called")
        }
    }
    assert_eq!(effective_base_url(&NoUrlProvider, &None), "");
}

#[test]
fn parse_finish_reason_openai_values() {
    assert_eq!(parse_finish_reason_openai("stop"), FinishReason::Stop);
    assert_eq!(
        parse_finish_reason_openai("length"),
        FinishReason::MaxTokens
    );
    assert_eq!(
        parse_finish_reason_openai("content_filter"),
        FinishReason::ContentFilter
    );
    assert_eq!(parse_finish_reason_openai("unknown"), FinishReason::Error);
}

// ─── secrecy ──────────────────────────────

#[test]
fn chat_request_debug_redacts_api_key() {
    let req = ChatRequest {
        provider: "openai".into(),
        model: "gpt-4o".into(),
        messages: vec![],
        system: None,
        max_tokens: None,
        temperature: None,
        top_p: None,
        stop: None,
        api_key: Some(SecretString::from("sk-secret-key-12345")),
        base_url: None,
        timeout_secs: 120,
        max_response_bytes: 10_485_760,
        extra: None,
    };
    let debug = format!("{:?}", req);
    assert!(
        !debug.contains("sk-secret-key-12345"),
        "api_key must not appear in Debug output: {debug}"
    );
    assert!(
        debug.contains("[REDACTED]"),
        "Debug output should show [REDACTED]: {debug}"
    );
}

#[test]
fn chat_request_debug_shows_none_when_no_key() {
    let req = ChatRequest {
        provider: "ollama".into(),
        model: "llama3.2".into(),
        messages: vec![],
        system: None,
        max_tokens: None,
        temperature: None,
        top_p: None,
        stop: None,
        api_key: None,
        base_url: None,
        timeout_secs: 120,
        max_response_bytes: 10_485_760,
        extra: None,
    };
    let debug = format!("{:?}", req);
    assert!(
        debug.contains("api_key: None"),
        "Debug output should show None: {debug}"
    );
}

// ─── HttpPolicy integration ──────────────────

#[test]
fn http_policy_blocks_llm_base_url() {
    use crate::policy::HttpAllowList;

    let lua = mlua::Lua::new();
    let config = crate::config::Config::builder()
        .http_policy(HttpAllowList::new(["localhost"]))
        .build()
        .unwrap();
    crate::register_all_with(&lua, "std", config).unwrap();

    // openai base_url is https://api.openai.com — not in HttpAllowList
    let result: mlua::Result<mlua::Value> = lua
        .load(
            r#"return std.llm.chat({
                provider = "openai",
                model = "gpt-4o",
                prompt = "hi",
            })"#,
        )
        .eval();
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("does not match any allowed host"),
        "should be HttpPolicy denial, got: {err}"
    );
}

#[test]
fn http_policy_allows_matching_llm_base_url() {
    use crate::policy::HttpAllowList;

    struct EchoProvider;
    impl LlmProvider for EchoProvider {
        fn name(&self) -> &str {
            "echo"
        }
        fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, String> {
            let text = match request.messages.first() {
                Some(msg) => match &msg.content {
                    ChatContent::Text(s) => s.clone(),
                    _ => "parts".into(),
                },
                None => "empty".into(),
            };
            Ok(ChatResponse {
                content: format!("echo: {text}"),
                finish_reason: FinishReason::Stop,
                usage: Usage::default(),
                model: request.model.clone(),
            })
        }
    }

    let lua = mlua::Lua::new();
    let config = crate::config::Config::builder()
        .http_policy(HttpAllowList::new(["custom-host.local"]))
        .build()
        .unwrap();
    crate::register_all_with(&lua, "std", config).unwrap();
    register_provider(&lua, EchoProvider).unwrap();

    // base_url matches HttpAllowList
    let content: String = lua
        .load(
            r#"
            local resp = std.llm.chat({
                provider = "echo",
                model = "test",
                prompt = "hello",
                base_url = "http://custom-host.local:8080",
            })
            return resp.content
        "#,
        )
        .eval()
        .unwrap();
    assert_eq!(content, "echo: hello");
}

// ─── sat_u32 ──────────────────────────────

#[test]
fn sat_u32_normal_values() {
    assert_eq!(sat_u32(0), 0);
    assert_eq!(sat_u32(1000), 1000);
    assert_eq!(sat_u32(u32::MAX as u64), u32::MAX);
}

#[test]
fn sat_u32_saturates_on_overflow() {
    assert_eq!(sat_u32(u32::MAX as u64 + 1), u32::MAX);
    assert_eq!(sat_u32(u64::MAX), u32::MAX);
}

// ─── opt_field type error propagation ──────

#[test]
fn chat_rejects_wrong_type_for_temperature() {
    let lua = mlua::Lua::new();
    crate::register_all(&lua, "std").unwrap();

    // temperature should be number, passing string must error
    let result: mlua::Result<mlua::Value> = lua
        .load(
            r#"return std.llm.chat({
                provider = "ollama",
                model = "x",
                prompt = "hi",
                temperature = "not a number",
            })"#,
        )
        .eval();
    assert!(result.is_err(), "string temperature should be rejected");
}

#[test]
fn chat_rejects_wrong_type_for_max_tokens() {
    let lua = mlua::Lua::new();
    crate::register_all(&lua, "std").unwrap();

    let result: mlua::Result<mlua::Value> = lua
        .load(
            r#"return std.llm.chat({
                provider = "ollama",
                model = "x",
                prompt = "hi",
                max_tokens = "thousand",
            })"#,
        )
        .eval();
    assert!(result.is_err(), "string max_tokens should be rejected");
}

#[test]
fn chat_rejects_wrong_type_for_system() {
    let lua = mlua::Lua::new();
    crate::register_all(&lua, "std").unwrap();

    let result: mlua::Result<mlua::Value> = lua
        .load(
            r#"return std.llm.chat({
                provider = "ollama",
                model = "x",
                prompt = "hi",
                system = 123,
            })"#,
        )
        .eval();
    assert!(result.is_err(), "numeric system should be rejected");
}

#[test]
fn chat_rejects_wrong_type_for_timeout() {
    let lua = mlua::Lua::new();
    crate::register_all(&lua, "std").unwrap();

    let result: mlua::Result<mlua::Value> = lua
        .load(
            r#"return std.llm.chat({
                provider = "ollama",
                model = "x",
                prompt = "hi",
                timeout = "slow",
            })"#,
        )
        .eval();
    assert!(result.is_err(), "string timeout should be rejected");
}

// ─── multimodal content parsing ──────────────────

#[test]
fn chat_with_multimodal_content() {
    struct EchoPartsProvider;
    impl LlmProvider for EchoPartsProvider {
        fn name(&self) -> &str {
            "echo-parts"
        }
        fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, String> {
            let desc = match &request.messages.first() {
                Some(msg) => match &msg.content {
                    ChatContent::Text(s) => format!("text:{s}"),
                    ChatContent::Parts(parts) => format!("parts:{}", parts.len()),
                },
                None => "empty".into(),
            };
            Ok(ChatResponse {
                content: desc,
                finish_reason: FinishReason::Stop,
                usage: Usage::default(),
                model: request.model.clone(),
            })
        }
    }

    let lua = mlua::Lua::new();
    crate::register_all(&lua, "std").unwrap();
    register_provider(&lua, EchoPartsProvider).unwrap();

    let content: String = lua
        .load(
            r#"
            local resp = std.llm.chat({
                provider = "echo-parts",
                model = "test",
                messages = {
                    {
                        role = "user",
                        content = {
                            { type = "text", text = "describe this" },
                            { type = "image_base64", data = "abc123", media_type = "image/png" },
                        },
                    },
                },
            })
            return resp.content
        "#,
        )
        .eval()
        .unwrap();
    assert_eq!(content, "parts:2");
}

#[test]
fn chat_rejects_unknown_content_part_type() {
    let lua = mlua::Lua::new();
    crate::register_all(&lua, "std").unwrap();

    let result: mlua::Result<mlua::Value> = lua
        .load(
            r#"return std.llm.chat({
                provider = "ollama",
                model = "x",
                messages = {
                    {
                        role = "user",
                        content = {
                            { type = "video", url = "http://example.com/v.mp4" },
                        },
                    },
                },
            })"#,
        )
        .eval();
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("unknown content part type"), "got: {err}");
}

// ─── stop field validation ──────────────────────

#[test]
fn chat_rejects_non_table_stop() {
    let lua = mlua::Lua::new();
    crate::register_all(&lua, "std").unwrap();

    let result: mlua::Result<mlua::Value> = lua
        .load(
            r#"return std.llm.chat({
                provider = "ollama",
                model = "x",
                prompt = "hi",
                stop = 42,
            })"#,
        )
        .eval();
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("stop must be a table"), "got: {err}");
}

// ─── extra field validation ──────────────────────

#[test]
fn chat_rejects_non_table_extra() {
    let lua = mlua::Lua::new();
    crate::register_all(&lua, "std").unwrap();

    let result: mlua::Result<mlua::Value> = lua
        .load(
            r#"return std.llm.chat({
                provider = "ollama",
                model = "x",
                prompt = "hi",
                extra = "not a table",
            })"#,
        )
        .eval();
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("extra must be a table"), "got: {err}");
}

// ─── Ollama image_url rejection ──────────────────

#[test]
fn ollama_serialize_rejects_image_url() {
    let msg = ChatMessage {
        role: ChatRole::User,
        content: ChatContent::Parts(vec![
            ContentPart::Text {
                text: "describe".into(),
            },
            ContentPart::ImageUrl {
                url: "http://example.com/img.png".into(),
            },
        ]),
    };
    let result = serialize_message_ollama(&msg);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("does not support image URLs"), "got: {err}");
}

// ─── check_api_error ─────────────────────────────

#[test]
fn check_api_error_openai_object_format() {
    let json = serde_json::json!({"error": {"message": "invalid api key"}});
    let err = check_api_error(&json, "openai").unwrap_err();
    assert_eq!(err, "openai: invalid api key");
}

#[test]
fn check_api_error_ollama_string_format() {
    let json = serde_json::json!({"error": "model not found"});
    let err = check_api_error(&json, "ollama").unwrap_err();
    assert_eq!(err, "ollama: model not found");
}

#[test]
fn check_api_error_unknown_error_shape() {
    // e.g. {"error": 42} — neither object nor string
    let json = serde_json::json!({"error": 42});
    let err = check_api_error(&json, "anthropic").unwrap_err();
    assert_eq!(err, "anthropic: unknown error");
}

#[test]
fn check_api_error_no_error_field() {
    let json = serde_json::json!({"choices": []});
    assert!(check_api_error(&json, "openai").is_ok());
}

// ─── agent_with_timeout ──────────────────────────

#[test]
fn agent_with_timeout_reuses_default() {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .build();
    let default = ureq::Agent::new_with_config(config);

    // Same timeout → should be the same agent (clone)
    let a = agent_with_timeout(&default, 60, 60);
    // Different timeout → new agent
    let _b = agent_with_timeout(&default, 60, 30);

    // We can't compare agents directly, but we can verify
    // no panic and both agents are functional.
    drop(a);
}
