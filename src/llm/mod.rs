//! LLM chat completion module.
//!
//! Provides a stable Lua-side interface for chat completion across
//! multiple LLM providers.  Built-in support for OpenAI, Anthropic,
//! and Ollama; extensible via the [`LlmProvider`] trait.
//!
//! # Policy layers
//!
//! Every LLM request passes through **two independent policy checks**:
//!
//! 1. [`HttpPolicy`](crate::policy::HttpPolicy) — **network-level** access
//!    control.  Validates the resolved base URL, identical to the check
//!    applied by `http.get` / `http.post`.  If `HttpPolicy` denies the
//!    URL, the request is rejected before any data leaves the process.
//!
//! 2. [`LlmPolicy`](crate::policy::LlmPolicy) — **AI-specific** access
//!    control.  Validates provider, model, and base URL.  This is a
//!    separate concern from HTTP access: data sent to external LLM
//!    providers may be used for model training, logged by the provider,
//!    or subject to different retention policies.  An operator may allow
//!    general HTTP access but restrict which AI providers receive data.
//!
//! Both policies must allow the request.  `HttpPolicy` is checked first.
//!
//! # Provider extensibility
//!
//! Implement [`LlmProvider`] to add a custom provider:
//!
//! ```rust,ignore
//! // `ignore`: `register_provider` requires a `Lua` instance with the
//! // LLM module already initialized (ProviderRegistry in app_data).
//! // A self-contained example would need full `register_all` setup,
//! // adding boilerplate that obscures the extension pattern.
//! use mlua_batteries::llm::{LlmProvider, ChatRequest, ChatResponse};
//!
//! struct MyProvider;
//!
//! impl LlmProvider for MyProvider {
//!     fn name(&self) -> &str { "my-provider" }
//!     fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, String> {
//!         todo!()
//!     }
//! }
//!
//! // After register_all:
//! mlua_batteries::llm::register_provider(&lua, MyProvider).unwrap();
//! ```
//!
//! # Lua API
//!
//! ```lua
//! local llm = std.llm
//!
//! -- Single request
//! local resp = llm.chat({
//!     provider = "openai",
//!     model    = "gpt-4o",
//!     prompt   = "Hello",
//!     system   = "You are helpful.",
//!     max_tokens  = 1024,
//!     temperature = 0.7,
//! })
//! -- resp.content, resp.finish_reason, resp.usage.input_tokens
//!
//! -- Batch parallel requests
//! local results = llm.batch({
//!     { provider = "ollama", model = "llama3.2", prompt = "Q1" },
//!     { provider = "openai", model = "gpt-4o",   prompt = "Q2" },
//! })
//! ```

mod anthropic;
mod helpers;
mod ollama;
mod openai;
mod parse;
mod types;

pub use anthropic::AnthropicProvider;
pub use ollama::OllamaProvider;
pub use openai::OpenAiProvider;
pub use types::*;

use mlua::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::util::{check_llm_request, check_url, with_config};
use helpers::effective_base_url;
use parse::{finish_reason_str, parse_lua_request, response_to_lua};

// ─── Provider registry ──────────────────────────

struct ProviderRegistry(HashMap<String, Arc<dyn LlmProvider>>);

/// Register a custom LLM provider.
///
/// Adds (or replaces) a provider in the registry stored in `lua.app_data`.
///
/// # Errors
///
/// Returns an error if the LLM module has not been initialized yet.
pub fn register_provider(lua: &Lua, provider: impl LlmProvider) -> LuaResult<()> {
    let mut registry = lua
        .app_data_mut::<ProviderRegistry>()
        .ok_or_else(|| LuaError::external("llm module not initialized"))?;
    registry
        .0
        .insert(provider.name().to_string(), Arc::new(provider));
    Ok(())
}

fn get_provider(lua: &Lua, name: &str) -> LuaResult<Arc<dyn LlmProvider>> {
    let registry = lua
        .app_data_ref::<ProviderRegistry>()
        .ok_or_else(|| LuaError::external("llm module not initialized"))?;
    registry
        .0
        .get(name)
        .cloned()
        .ok_or_else(|| LuaError::external(format!("unknown LLM provider: '{name}'")))
}

// ─── Module ──────────────────────────────

/// Validate policy checks and resolve the base URL for a single LLM request.
///
/// Shared by `chat` and `batch` to avoid duplicating policy check logic.
fn prepare_request(lua: &Lua, opts: &LuaTable) -> LuaResult<(ChatRequest, Arc<dyn LlmProvider>)> {
    let mut req = parse_lua_request(lua, opts)?;
    let provider = get_provider(lua, &req.provider)?;
    let base_url = effective_base_url(provider.as_ref(), &req.base_url);
    if !base_url.is_empty() {
        check_url(lua, base_url, "POST")?;
    }
    check_llm_request(lua, &req.provider, &req.model, base_url)?;
    // Inject resolved URL so provider.chat() doesn't re-resolve.
    if req.base_url.is_none() {
        if let Some(url) = provider.default_base_url() {
            if !url.is_empty() {
                req.base_url = Some(url.to_string());
            }
        }
    }
    Ok((req, provider))
}

pub fn module(lua: &Lua) -> LuaResult<LuaTable> {
    if lua.app_data_ref::<ProviderRegistry>().is_none() {
        let timeout = with_config(lua, |c| c.llm_default_timeout_secs)?;
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(timeout)))
            .build();
        let shared_agent = ureq::Agent::new_with_config(config);

        let mut providers: HashMap<String, Arc<dyn LlmProvider>> = HashMap::new();
        providers.insert(
            "openai".into(),
            Arc::new(OpenAiProvider::with_agent(shared_agent.clone(), timeout)),
        );
        providers.insert(
            "anthropic".into(),
            Arc::new(AnthropicProvider::with_agent(shared_agent.clone(), timeout)),
        );
        providers.insert(
            "ollama".into(),
            Arc::new(OllamaProvider::with_agent(shared_agent, timeout)),
        );
        lua.set_app_data(ProviderRegistry(providers));
    }

    let t = lua.create_table()?;

    t.set(
        "chat",
        lua.create_function(|lua, opts: LuaTable| {
            let (req, provider) = prepare_request(lua, &opts)?;
            let resp = provider.chat(&req).map_err(LuaError::external)?;
            response_to_lua(lua, &resp)
        })?,
    )?;

    t.set(
        "batch",
        lua.create_function(|lua, requests: LuaTable| {
            let mut batch: Vec<ChatRequest> = Vec::new();
            let mut providers: Vec<Arc<dyn LlmProvider>> = Vec::new();
            for entry in requests.sequence_values::<LuaTable>() {
                let opts = entry?;
                let (req, provider) = prepare_request(lua, &opts)?;
                batch.push(req);
                providers.push(provider);
            }

            if batch.is_empty() {
                return lua.create_table().map(LuaValue::Table);
            }

            let max_conc = with_config(lua, |c| c.llm_max_batch_concurrency)?;
            let responses = batch_call(&batch, &providers, max_conc);

            let results = lua.create_table()?;
            for (i, resp) in responses.into_iter().enumerate() {
                let entry = lua.create_table()?;
                match resp {
                    Ok(r) => {
                        entry.set("content", r.content.as_str())?;
                        entry.set("finish_reason", finish_reason_str(&r.finish_reason))?;
                        let usage = lua.create_table()?;
                        usage.set("input_tokens", r.usage.input_tokens)?;
                        usage.set("output_tokens", r.usage.output_tokens)?;
                        entry.set("usage", usage)?;
                        entry.set("model", r.model.as_str())?;
                    }
                    Err(e) => {
                        entry.set("error", e.as_str())?;
                    }
                }
                results.set(i + 1, entry)?;
            }
            Ok(LuaValue::Table(results))
        })?,
    )?;

    Ok(t)
}

// ─── Batch execution ──────────────────────────

fn batch_call(
    batch: &[ChatRequest],
    providers: &[Arc<dyn LlmProvider>],
    max_concurrency: usize,
) -> Vec<Result<ChatResponse, String>> {
    let mut results: Vec<Result<ChatResponse, String>> = Vec::with_capacity(batch.len());
    let pairs: Vec<_> = batch.iter().zip(providers.iter()).collect();

    for chunk in pairs.chunks(max_concurrency) {
        std::thread::scope(|s| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|(req, provider)| {
                    let provider = Arc::clone(provider);
                    s.spawn(move || provider.chat(req))
                })
                .collect();

            for handle in handles {
                results.push(handle.join().unwrap_or_else(|_| Err("thread panic".into())));
            }
        });
    }

    results
}

#[cfg(test)]
mod tests;
