//! Lua request parsing and response conversion.

use mlua::prelude::*;

use super::types::*;
use crate::util::with_config;

/// Parse a Lua table into a [`ChatRequest`].
pub(super) fn parse_lua_request(lua: &Lua, opts: &LuaTable) -> LuaResult<ChatRequest> {
    let provider: String = opts.get("provider")?;
    let model: String = opts.get("model")?;

    // messages or prompt (mutually exclusive, messages takes precedence)
    let messages = if let Ok(msgs) = opts.get::<LuaTable>("messages") {
        parse_messages(&msgs)?
    } else if let Ok(prompt) = opts.get::<String>("prompt") {
        vec![ChatMessage {
            role: ChatRole::User,
            content: ChatContent::Text(prompt),
        }]
    } else {
        return Err(LuaError::external(
            "either 'prompt' or 'messages' is required",
        ));
    };

    let system: Option<String> = opt_field(lua, opts, "system")?;
    let max_tokens: Option<u32> = opt_field(lua, opts, "max_tokens")?;
    let temperature: Option<f64> = opt_field(lua, opts, "temperature")?;
    let top_p: Option<f64> = opt_field(lua, opts, "top_p")?;
    let api_key: Option<secrecy::SecretString> =
        opt_field::<String>(lua, opts, "api_key")?.map(secrecy::SecretString::from);
    let base_url: Option<String> = opt_field(lua, opts, "base_url")?;

    let stop: Option<Vec<String>> = match opts.get::<LuaValue>("stop")? {
        LuaValue::Nil => None,
        LuaValue::Table(t) => {
            let mut v = Vec::new();
            for entry in t.sequence_values::<String>() {
                v.push(entry?);
            }
            Some(v)
        }
        other => {
            return Err(LuaError::external(format!(
                "stop must be a table of strings, got {}",
                other.type_name()
            )));
        }
    };

    let (default_timeout, max_resp_bytes, max_json_depth) = with_config(lua, |c| {
        (
            c.llm_default_timeout_secs,
            c.llm_max_response_bytes,
            c.max_json_depth,
        )
    })?;
    let timeout_secs: u64 = opt_field(lua, opts, "timeout")?.unwrap_or(default_timeout);

    let extra: Option<serde_json::Value> = match opts.get::<LuaValue>("extra")? {
        LuaValue::Nil => None,
        val @ LuaValue::Table(_) => Some(crate::json::lua_to_json(&val, max_json_depth)?),
        other => {
            return Err(LuaError::external(format!(
                "extra must be a table, got {}",
                other.type_name()
            )));
        }
    };

    Ok(ChatRequest {
        provider,
        model,
        messages,
        system,
        max_tokens,
        temperature,
        top_p,
        stop,
        api_key,
        base_url,
        timeout_secs,
        max_response_bytes: max_resp_bytes,
        extra,
    })
}

fn parse_messages(msgs: &LuaTable) -> LuaResult<Vec<ChatMessage>> {
    let mut result = Vec::new();
    for entry in msgs.sequence_values::<LuaTable>() {
        let msg = entry?;
        let role_str: String = msg.get("role")?;
        let role = match role_str.as_str() {
            "user" => ChatRole::User,
            "assistant" => ChatRole::Assistant,
            other => {
                return Err(LuaError::external(format!(
                    "unsupported message role: '{other}' (use 'user' or 'assistant'; \
                     for system prompts use the top-level 'system' field)"
                )));
            }
        };

        let content_val: LuaValue = msg.get("content")?;
        let content = parse_content(content_val)?;
        result.push(ChatMessage { role, content });
    }
    Ok(result)
}

fn parse_content(value: LuaValue) -> LuaResult<ChatContent> {
    match value {
        LuaValue::String(s) => Ok(ChatContent::Text(s.to_str()?.to_string())),
        LuaValue::Table(t) => {
            let mut parts = Vec::new();
            for entry in t.sequence_values::<LuaTable>() {
                let part = entry?;
                let part_type: String = part.get("type")?;
                match part_type.as_str() {
                    "text" => {
                        let text: String = part.get("text")?;
                        parts.push(ContentPart::Text { text });
                    }
                    "image_url" => {
                        let url: String = part.get("url")?;
                        parts.push(ContentPart::ImageUrl { url });
                    }
                    "image_base64" => {
                        let data: String = part.get("data")?;
                        let media_type: String = part.get("media_type")?;
                        parts.push(ContentPart::ImageBase64 { data, media_type });
                    }
                    other => {
                        return Err(LuaError::external(format!(
                            "unknown content part type: '{other}'"
                        )));
                    }
                }
            }
            Ok(ChatContent::Parts(parts))
        }
        other => Err(LuaError::external(format!(
            "message content must be string or table, got {}",
            other.type_name()
        ))),
    }
}

/// Convert a [`ChatResponse`] to a Lua table.
pub(super) fn response_to_lua(lua: &Lua, resp: &ChatResponse) -> LuaResult<LuaValue> {
    let t = lua.create_table()?;
    t.set("content", resp.content.as_str())?;
    t.set("finish_reason", finish_reason_str(&resp.finish_reason))?;
    let usage = lua.create_table()?;
    usage.set("input_tokens", resp.usage.input_tokens)?;
    usage.set("output_tokens", resp.usage.output_tokens)?;
    t.set("usage", usage)?;
    t.set("model", resp.model.as_str())?;
    Ok(LuaValue::Table(t))
}

pub(super) fn finish_reason_str(r: &FinishReason) -> &'static str {
    match r {
        FinishReason::Stop => "stop",
        FinishReason::MaxTokens => "max_tokens",
        FinishReason::ContentFilter => "content_filter",
        FinishReason::Error => "error",
    }
}

/// Read an optional typed field from a Lua table.
///
/// Returns `Ok(None)` when the key is absent or `nil`.
/// Returns `Err` when the key is present but has the wrong type,
/// so callers get a clear error instead of silent `None`.
pub(super) fn opt_field<T: mlua::FromLua>(
    lua: &Lua,
    opts: &LuaTable,
    key: &str,
) -> LuaResult<Option<T>> {
    match opts.get::<LuaValue>(key)? {
        LuaValue::Nil => Ok(None),
        val => T::from_lua(val, lua).map(Some),
    }
}
