//! Public types for the LLM chat completion module.

use secrecy::SecretString;

/// Chat message role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
}

/// Content of a chat message.
#[derive(Debug, Clone)]
pub enum ChatContent {
    /// Plain text.
    Text(String),
    /// Multimodal content (text + images).
    Parts(Vec<ContentPart>),
}

/// A single part of multimodal content.
#[derive(Debug, Clone)]
pub enum ContentPart {
    /// Text segment.
    Text { text: String },
    /// Image from URL.
    ImageUrl { url: String },
    /// Image from base64-encoded data.
    ImageBase64 { data: String, media_type: String },
}

/// Chat completion request.
///
/// The `api_key` field is wrapped in [`SecretString`] to prevent
/// accidental exposure in logs or debug output.  Use
/// `expose_secret()` when the plaintext
/// value is needed (e.g. for HTTP headers).
///
/// # `extra` field — provider-specific merge behaviour
///
/// The `extra` table is merged into the outgoing request body, but
/// the merge target differs by provider:
///
/// | Provider | Merge target | Example use |
/// |----------|-------------|-------------|
/// | OpenAI | Top-level body | `{response_format = {type = "json_object"}}` |
/// | Anthropic | Top-level body | `{metadata = {user_id = "u123"}}` |
/// | Ollama | `options` object | `{num_ctx = 4096, mirostat = 1}` |
///
/// Ollama uses an `options` sub-object for model parameters, so
/// `extra` fields are merged there instead of the top level.
#[derive(Clone)]
pub struct ChatRequest {
    pub provider: String,
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub system: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub stop: Option<Vec<String>>,
    pub api_key: Option<SecretString>,
    pub base_url: Option<String>,
    pub timeout_secs: u64,
    pub max_response_bytes: u64,
    pub extra: Option<serde_json::Value>,
}

impl std::fmt::Debug for ChatRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatRequest")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("messages", &self.messages)
            .field("system", &self.system)
            .field("max_tokens", &self.max_tokens)
            .field("temperature", &self.temperature)
            .field("top_p", &self.top_p)
            .field("stop", &self.stop)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("base_url", &self.base_url)
            .field("timeout_secs", &self.timeout_secs)
            .field("max_response_bytes", &self.max_response_bytes)
            .field("extra", &self.extra)
            .finish()
    }
}

/// A single message in the chat history.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: ChatContent,
}

/// Reason the model stopped generating.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    MaxTokens,
    ContentFilter,
    Error,
}

/// Token usage information.
#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Chat completion response.
#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub content: String,
    pub finish_reason: FinishReason,
    pub usage: Usage,
    pub model: String,
}

/// LLM provider for chat completion.
///
/// Implement this trait to add support for a custom LLM API.
/// Built-in: [`OpenAiProvider`](super::OpenAiProvider),
/// [`AnthropicProvider`](super::AnthropicProvider),
/// [`OllamaProvider`](super::OllamaProvider).
pub trait LlmProvider: Send + Sync + 'static {
    /// Provider identifier (must match Lua `provider = "..."` value).
    fn name(&self) -> &str;

    /// Default base URL for this provider (e.g. `"https://api.openai.com"`).
    ///
    /// Used by the module-level policy check to validate the request URL
    /// via [`HttpPolicy`](crate::policy::HttpPolicy) before dispatching.
    /// Return `None` if the provider has no fixed base URL.
    fn default_base_url(&self) -> Option<&str> {
        None
    }

    /// Execute a chat completion request.
    fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, String>;
}
