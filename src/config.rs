//! Runtime configuration for mlua-batteries modules.
//!
//! Use [`Config::builder`] to customise behaviour, or
//! [`Config::default`] for unrestricted defaults.
//!
//! ```rust,ignore
//! // Requires the `sandbox` feature.
//! use std::time::Duration;
//! use mlua_batteries::config::Config;
//! use mlua_batteries::policy::Sandboxed;
//!
//! let config = Config::builder()
//!     .path_policy(Sandboxed::new(["/app/data"]).unwrap())
//!     .max_walk_depth(50)
//!     .http_timeout(Duration::from_secs(60))
//!     .build()
//!     .expect("invalid config");
//! ```

use std::time::Duration;

use crate::policy::{EnvPolicy, HttpPolicy, LlmPolicy, PathPolicy, Unrestricted};

/// Error returned by [`ConfigBuilder::build`] for invalid configuration values.
#[derive(Debug, Clone)]
pub struct ConfigError(String);

impl ConfigError {
    /// Create a new configuration error.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    /// The error message.
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

/// Central configuration for all mlua-batteries modules.
///
/// Contains trait-object policies and numeric limits. `Debug` prints
/// the numeric limits (policy trait objects are omitted).
pub struct Config {
    pub(crate) path_policy: Box<dyn PathPolicy>,
    pub(crate) http_policy: Box<dyn HttpPolicy>,
    pub(crate) env_policy: Box<dyn EnvPolicy>,
    pub(crate) llm_policy: Box<dyn LlmPolicy>,
    pub(crate) max_walk_depth: usize,
    pub(crate) max_walk_entries: usize,
    pub(crate) max_json_depth: usize,
    pub(crate) http_timeout: Duration,
    pub(crate) max_response_bytes: u64,
    pub(crate) max_sleep_secs: f64,
    pub(crate) llm_default_timeout_secs: u64,
    pub(crate) llm_max_response_bytes: u64,
    pub(crate) llm_max_batch_concurrency: usize,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("path_policy", &self.path_policy.policy_name())
            .field("http_policy", &self.http_policy.policy_name())
            .field("env_policy", &self.env_policy.policy_name())
            .field("llm_policy", &self.llm_policy.policy_name())
            .field("max_walk_depth", &self.max_walk_depth)
            .field("max_walk_entries", &self.max_walk_entries)
            .field("max_json_depth", &self.max_json_depth)
            .field("http_timeout", &self.http_timeout)
            .field("max_response_bytes", &self.max_response_bytes)
            .field("max_sleep_secs", &self.max_sleep_secs)
            .field("llm_default_timeout_secs", &self.llm_default_timeout_secs)
            .field("llm_max_response_bytes", &self.llm_max_response_bytes)
            .field("llm_max_batch_concurrency", &self.llm_max_batch_concurrency)
            .finish()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            path_policy: Box::new(Unrestricted),
            http_policy: Box::new(Unrestricted),
            env_policy: Box::new(Unrestricted),
            llm_policy: Box::new(Unrestricted),
            max_walk_depth: 256,
            max_walk_entries: 10_000,
            max_json_depth: 128,
            http_timeout: Duration::from_secs(30),
            max_response_bytes: 10 * 1024 * 1024, // 10 MiB
            max_sleep_secs: 86_400.0,
            llm_default_timeout_secs: 120,
            llm_max_response_bytes: 10 * 1024 * 1024, // 10 MiB
            llm_max_batch_concurrency: 8,
        }
    }
}

impl Config {
    /// Start building a custom configuration.
    pub fn builder() -> ConfigBuilder {
        ConfigBuilder {
            inner: Config::default(),
        }
    }
}

/// Builder for [`Config`].
pub struct ConfigBuilder {
    inner: Config,
}

impl ConfigBuilder {
    /// Set the path access policy.
    ///
    /// Default: [`Unrestricted`] (no checks).
    pub fn path_policy(mut self, policy: impl PathPolicy) -> Self {
        self.inner.path_policy = Box::new(policy);
        self
    }

    /// Set the HTTP URL access policy.
    ///
    /// Default: [`Unrestricted`] (no checks).
    pub fn http_policy(mut self, policy: impl HttpPolicy) -> Self {
        self.inner.http_policy = Box::new(policy);
        self
    }

    /// Set the environment variable access policy.
    ///
    /// Default: [`Unrestricted`] (no checks).
    pub fn env_policy(mut self, policy: impl EnvPolicy) -> Self {
        self.inner.env_policy = Box::new(policy);
        self
    }

    /// Set the LLM request policy.
    ///
    /// Default: [`Unrestricted`] (no checks).
    pub fn llm_policy(mut self, policy: impl LlmPolicy) -> Self {
        self.inner.llm_policy = Box::new(policy);
        self
    }

    /// Default timeout for LLM requests in seconds.
    ///
    /// Default: `120`.
    pub fn llm_default_timeout_secs(mut self, secs: u64) -> Self {
        self.inner.llm_default_timeout_secs = secs;
        self
    }

    /// Maximum LLM response body size in bytes.
    ///
    /// Default: `10_485_760` (10 MiB).
    pub fn llm_max_response_bytes(mut self, bytes: u64) -> Self {
        self.inner.llm_max_response_bytes = bytes;
        self
    }

    /// Maximum number of concurrent threads for `llm.batch`.
    ///
    /// Default: `8`.
    pub fn llm_max_batch_concurrency(mut self, n: usize) -> Self {
        self.inner.llm_max_batch_concurrency = n;
        self
    }

    /// Maximum directory depth for `fs.walk`.
    ///
    /// Default: `256`.
    pub fn max_walk_depth(mut self, depth: usize) -> Self {
        self.inner.max_walk_depth = depth;
        self
    }

    /// Maximum number of entries returned by `fs.walk` and `fs.glob`.
    ///
    /// Default: `10_000`.
    pub fn max_walk_entries(mut self, entries: usize) -> Self {
        self.inner.max_walk_entries = entries;
        self
    }

    /// Maximum nesting depth for JSON encode/decode.
    ///
    /// Default: `128`.
    pub fn max_json_depth(mut self, depth: usize) -> Self {
        self.inner.max_json_depth = depth;
        self
    }

    /// Default timeout for HTTP requests.
    ///
    /// Default: `30` seconds.
    pub fn http_timeout(mut self, timeout: Duration) -> Self {
        self.inner.http_timeout = timeout;
        self
    }

    /// Maximum HTTP response body size in bytes.
    ///
    /// Default: `10_485_760` (10 MiB).
    pub fn max_response_bytes(mut self, bytes: u64) -> Self {
        self.inner.max_response_bytes = bytes;
        self
    }

    /// Maximum duration for `time.sleep` in seconds.
    ///
    /// Default: `86400.0` (1 day).
    ///
    /// [`build`](ConfigBuilder::build) returns an error if the value is
    /// NaN, infinite, or negative.
    pub fn max_sleep_secs(mut self, secs: f64) -> Self {
        self.inner.max_sleep_secs = secs;
        self
    }

    /// Finalise the configuration.
    ///
    /// Returns `Err` if any configured value is invalid.
    pub fn build(self) -> Result<Config, ConfigError> {
        let secs = self.inner.max_sleep_secs;
        if !secs.is_finite() || secs < 0.0 {
            return Err(ConfigError::new(format!(
                "max_sleep_secs must be finite and non-negative, got {secs}"
            )));
        }
        Ok(self.inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "sandbox")]
    use crate::policy::Sandboxed;

    #[test]
    fn default_config_values() {
        let config = Config::default();
        assert_eq!(config.max_walk_depth, 256);
        assert_eq!(config.max_walk_entries, 10_000);
        assert_eq!(config.max_json_depth, 128);
        assert_eq!(config.http_timeout, Duration::from_secs(30));
        assert_eq!(config.max_response_bytes, 10 * 1024 * 1024);
        assert!((config.max_sleep_secs - 86_400.0).abs() < f64::EPSILON);
        assert_eq!(config.llm_default_timeout_secs, 120);
        assert_eq!(config.llm_max_response_bytes, 10 * 1024 * 1024);
        assert_eq!(config.llm_max_batch_concurrency, 8);
    }

    #[test]
    fn builder_overrides() {
        let config = Config::builder()
            .max_walk_depth(10)
            .max_walk_entries(500)
            .max_json_depth(32)
            .http_timeout(Duration::from_secs(5))
            .max_response_bytes(1024)
            .max_sleep_secs(60.0)
            .build()
            .unwrap();

        assert_eq!(config.max_walk_depth, 10);
        assert_eq!(config.max_walk_entries, 500);
        assert_eq!(config.max_json_depth, 32);
        assert_eq!(config.http_timeout, Duration::from_secs(5));
        assert_eq!(config.max_response_bytes, 1024);
        assert!((config.max_sleep_secs - 60.0).abs() < f64::EPSILON);
    }

    #[cfg(feature = "sandbox")]
    #[test]
    fn builder_accepts_custom_policy() {
        let config = Config::builder()
            .path_policy(Sandboxed::new(["/tmp"]).unwrap())
            .build()
            .unwrap();

        // Verify it compiles and builds — policy behaviour tested in policy.rs
        assert_eq!(config.max_walk_depth, 256); // other defaults preserved
    }

    #[test]
    fn builder_rejects_nan_sleep() {
        let result = Config::builder().max_sleep_secs(f64::NAN).build();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("max_sleep_secs must be finite and non-negative"));
    }

    #[test]
    fn builder_rejects_infinite_sleep() {
        let result = Config::builder().max_sleep_secs(f64::INFINITY).build();
        assert!(result.is_err());
    }

    #[test]
    fn builder_rejects_negative_sleep() {
        let result = Config::builder().max_sleep_secs(-1.0).build();
        assert!(result.is_err());
    }

    #[test]
    fn config_debug_does_not_panic() {
        let config = Config::default();
        let s = format!("{config:?}");
        assert!(s.contains("max_walk_depth"));
        assert!(
            s.contains("Unrestricted"),
            "Debug should show policy type names, got: {s}"
        );
    }
}
