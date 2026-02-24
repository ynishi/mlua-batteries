//! Environment variable access policy.

use std::collections::HashSet;

use super::{PolicyError, Unrestricted};

/// Policy that decides whether a given environment variable may be accessed.
///
/// Every function in the `env` module calls [`EnvPolicy::check_get`] or
/// [`EnvPolicy::check_set`] before accessing a variable.
///
/// # Built-in implementations
///
/// | Type | Behaviour |
/// |------|-----------|
/// | [`Unrestricted`] | No checks (default) |
/// | [`EnvAllowList`] | Allow only listed variable names |
///
/// # Custom implementations
///
/// ```rust,no_run
/// use mlua_batteries::policy::{EnvPolicy, PolicyError};
///
/// struct DenySecrets;
///
/// impl EnvPolicy for DenySecrets {
///     fn check_get(&self, key: &str) -> Result<(), PolicyError> {
///         if key.contains("SECRET") || key.contains("TOKEN") {
///             Err(PolicyError::new(format!("read denied: env var '{key}' looks sensitive")))
///         } else {
///             Ok(())
///         }
///     }
///     fn check_set(&self, key: &str) -> Result<(), PolicyError> {
///         self.check_get(key) // same rules for set
///     }
/// }
/// ```
pub trait EnvPolicy: Send + Sync + 'static {
    /// Human-readable name for this policy, used in `Debug` output.
    ///
    /// The default implementation returns [`std::any::type_name`] of the
    /// concrete type, which works correctly even through trait objects
    /// because the vtable dispatches to the concrete implementation.
    fn policy_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    /// Validate read access to env var `key`.
    ///
    /// Return `Ok(())` to allow, `Err(reason)` to deny.
    fn check_get(&self, key: &str) -> Result<(), PolicyError>;

    /// Validate write access (overlay set) to env var `key`.
    ///
    /// Return `Ok(())` to allow, `Err(reason)` to deny.
    fn check_set(&self, key: &str) -> Result<(), PolicyError>;
}

impl EnvPolicy for Unrestricted {
    fn check_get(&self, _key: &str) -> Result<(), PolicyError> {
        Ok(())
    }
    fn check_set(&self, _key: &str) -> Result<(), PolicyError> {
        Ok(())
    }
}

/// Allow access only to listed environment variable names.
///
/// ```rust,no_run
/// use mlua_batteries::policy::EnvAllowList;
///
/// let policy = EnvAllowList::new(["HOME", "PATH", "LANG"]);
/// ```
#[derive(Debug)]
pub struct EnvAllowList {
    allowed_keys: HashSet<String>,
    allow_set: bool,
}

impl EnvAllowList {
    /// Create an allow-list for the given variable names.
    ///
    /// By default, `set` is allowed for variables in the list.
    pub fn new<I, S>(keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            allowed_keys: keys.into_iter().map(Into::into).collect::<HashSet<_>>(),
            allow_set: true,
        }
    }

    /// Deny all `env.set()` calls regardless of key.
    pub fn read_only(mut self) -> Self {
        self.allow_set = false;
        self
    }
}

impl EnvPolicy for EnvAllowList {
    fn check_get(&self, key: &str) -> Result<(), PolicyError> {
        if self.allowed_keys.contains(key) {
            Ok(())
        } else {
            Err(PolicyError::new(format!(
                "read denied: env var '{key}' is not in the allow list"
            )))
        }
    }

    fn check_set(&self, key: &str) -> Result<(), PolicyError> {
        if !self.allow_set {
            return Err(PolicyError::new(format!(
                "set denied: env is read-only (key '{key}')"
            )));
        }
        if self.allowed_keys.contains(key) {
            Ok(())
        } else {
            Err(PolicyError::new(format!(
                "set denied: env var '{key}' is not in the allow list"
            )))
        }
    }
}
