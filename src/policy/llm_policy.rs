//! LLM request access policy.

use std::collections::HashSet;

use super::{PolicyError, Unrestricted};

/// Policy that decides whether a given LLM request may be sent.
///
/// Called by the `llm` module before dispatching to a
/// [`LlmProvider`](crate::llm::LlmProvider).
///
/// # Relationship to `HttpPolicy`
///
/// LLM requests are external HTTP calls, so they pass through **both**
/// [`HttpPolicy`](super::HttpPolicy) and `LlmPolicy`:
///
/// - [`HttpPolicy`](super::HttpPolicy) — network-level: "is this URL reachable?"
///   Checked first against the resolved base URL.
/// - `LlmPolicy` — AI-specific: "should data be sent to this provider?"
///   Addresses concerns that do not apply to general HTTP: data may be
///   used for model training, subject to provider-specific retention
///   policies, or expose sensitive context to a third-party AI system.
///
/// Both policies must allow the request for it to proceed.
///
/// # Built-in implementations
///
/// | Type | Behaviour |
/// |------|-----------|
/// | [`Unrestricted`] | No checks (default) |
/// | [`LlmAllowList`] | Allow only listed providers |
///
/// # Custom implementations
///
/// ```rust,no_run
/// use mlua_batteries::policy::{LlmPolicy, PolicyError};
///
/// struct OnlyLocal;
///
/// impl LlmPolicy for OnlyLocal {
///     fn check_request(&self, _provider: &str, _model: &str, base_url: &str) -> Result<(), PolicyError> {
///         if base_url.contains("localhost") || base_url.contains("127.0.0.1") {
///             Ok(())
///         } else {
///             Err(PolicyError::new(format!("LLM denied: only local endpoints allowed, got '{base_url}'")))
///         }
///     }
/// }
/// ```
pub trait LlmPolicy: Send + Sync + 'static {
    /// Human-readable name for this policy, used in `Debug` output.
    ///
    /// The default implementation returns [`std::any::type_name`] of the
    /// concrete type, which works correctly even through trait objects
    /// because the vtable dispatches to the concrete implementation.
    fn policy_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    /// Validate an LLM request before it is sent.
    ///
    /// `provider` is the provider name (e.g. `"openai"`), `model` is the
    /// model identifier, `base_url` is the resolved API base URL.
    ///
    /// Return `Ok(())` to allow, `Err(reason)` to deny.
    fn check_request(&self, provider: &str, model: &str, base_url: &str)
        -> Result<(), PolicyError>;
}

impl LlmPolicy for Unrestricted {
    fn check_request(
        &self,
        _provider: &str,
        _model: &str,
        _base_url: &str,
    ) -> Result<(), PolicyError> {
        Ok(())
    }
}

/// Allow only requests to listed LLM providers.
///
/// ```rust,no_run
/// use mlua_batteries::policy::LlmAllowList;
///
/// let policy = LlmAllowList::new(["ollama", "openai"]);
/// ```
#[derive(Debug)]
pub struct LlmAllowList {
    allowed_providers: HashSet<String>,
}

impl LlmAllowList {
    /// Create an allow-list from provider names.
    pub fn new<I, S>(providers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            allowed_providers: providers
                .into_iter()
                .map(Into::into)
                .collect::<HashSet<_>>(),
        }
    }
}

impl LlmPolicy for LlmAllowList {
    fn check_request(
        &self,
        provider: &str,
        _model: &str,
        _base_url: &str,
    ) -> Result<(), PolicyError> {
        if self.allowed_providers.contains(provider) {
            Ok(())
        } else {
            Err(PolicyError::new(format!(
                "LLM denied: provider '{provider}' is not in the allow list"
            )))
        }
    }
}
