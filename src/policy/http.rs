//! HTTP URL access policy.

use super::{PolicyError, Unrestricted};

/// Policy that decides whether a given URL may be accessed.
///
/// Every function in the `http` module calls [`HttpPolicy::check_url`]
/// before making a request.
///
/// # Built-in implementations
///
/// | Type | Behaviour |
/// |------|-----------|
/// | [`Unrestricted`] | No checks (default) |
/// | [`HttpAllowList`] | Allow only listed host patterns |
///
/// # Custom implementations
///
/// ```rust,no_run
/// use mlua_batteries::policy::{HttpPolicy, PolicyError};
///
/// struct BlockInternal;
///
/// impl HttpPolicy for BlockInternal {
///     fn check_url(&self, url: &str, method: &str) -> Result<(), PolicyError> {
///         if url.contains("169.254.") || url.contains("localhost") {
///             Err(PolicyError::new(format!("{method} denied: internal URL '{url}'")))
///         } else {
///             Ok(())
///         }
///     }
/// }
/// ```
pub trait HttpPolicy: Send + Sync + 'static {
    /// Human-readable name for this policy, used in `Debug` output.
    ///
    /// The default implementation returns [`std::any::type_name`] of the
    /// concrete type, which works correctly even through trait objects
    /// because the vtable dispatches to the concrete implementation.
    fn policy_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    /// Validate `url` for `method` (e.g. "GET", "POST").
    ///
    /// Return `Ok(())` to allow, `Err(reason)` to deny.
    fn check_url(&self, url: &str, method: &str) -> Result<(), PolicyError>;
}

impl HttpPolicy for Unrestricted {
    fn check_url(&self, _url: &str, _method: &str) -> Result<(), PolicyError> {
        Ok(())
    }
}

/// Allow only requests to hosts matching the given patterns.
///
/// Matching is performed against the **host portion** of the URL only.
/// The URL is parsed to extract the host (stripping scheme, userinfo,
/// port, path, query, and fragment) before matching.
///
/// Patterns are matched as exact or suffix of the host — e.g.
/// `"example.com"` matches `https://example.com/path` and
/// `https://api.example.com/path` but does **not** match
/// `https://notexample.com/path` or `https://evil.com/?ref=example.com`.
///
/// # Security
///
/// Previous versions matched against the full URL string, which allowed
/// bypass via query parameters or path segments. This implementation
/// extracts the host and matches only against it.
///
/// ```rust,no_run
/// use mlua_batteries::policy::HttpAllowList;
///
/// let policy = HttpAllowList::new(["api.example.com", "httpbin.org"]);
/// ```
#[derive(Debug)]
pub struct HttpAllowList {
    allowed_hosts: Vec<String>,
}

impl HttpAllowList {
    /// Create an allow-list from host patterns.
    pub fn new<I, S>(hosts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            allowed_hosts: hosts.into_iter().map(Into::into).collect(),
        }
    }
}

impl HttpPolicy for HttpAllowList {
    fn check_url(&self, url: &str, method: &str) -> Result<(), PolicyError> {
        let host = extract_url_host(url).unwrap_or("");
        if self
            .allowed_hosts
            .iter()
            .any(|pattern| host_matches(host, pattern))
        {
            Ok(())
        } else {
            Err(PolicyError::new(format!(
                "{method} denied: URL '{url}' does not match any allowed host"
            )))
        }
    }
}

/// Check if `host` matches `pattern` by exact match or as a subdomain.
///
/// `"example.com"` matches `"example.com"` and `"sub.example.com"`
/// but **not** `"notexample.com"`.
///
/// Uses zero-allocation byte comparison instead of `format!`.
fn host_matches(host: &str, pattern: &str) -> bool {
    host == pattern
        || (host.len() > pattern.len()
            && host.as_bytes()[host.len() - pattern.len() - 1] == b'.'
            && host.ends_with(pattern))
}

/// Extract the host portion from a URL string.
///
/// Handles the standard URL format: `scheme://[userinfo@]host[:port]/path...`
///
/// - Strips scheme (`http://`, `https://`)
/// - Strips userinfo (`user:pass@`)
/// - Strips port (`:8080`)
/// - Strips path, query, and fragment
/// - Handles IPv6 addresses (`[::1]`)
///
/// Returns `None` if the URL has no `://` separator.
pub(super) fn extract_url_host(url: &str) -> Option<&str> {
    let after_scheme = url.find("://").map(|i| i + 3)?;
    let rest = &url[after_scheme..];

    // Authority ends at the first `/`, `?`, or `#`
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];

    // Strip userinfo (everything before the last `@`)
    let host_start = authority.rfind('@').map(|i| i + 1).unwrap_or(0);
    let host_part = &authority[host_start..];

    if host_part.starts_with('[') {
        // IPv6: [::1]:8080 → ::1
        host_part.find(']').map(|i| &host_part[1..i])
    } else {
        // Strip port: example.com:8080 → example.com
        Some(host_part.split(':').next().unwrap_or(host_part))
    }
}
