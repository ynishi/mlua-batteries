//! Path access policy for sandboxing filesystem operations.
//!
//! Implement [`PathPolicy`] to control which paths Lua scripts can access.
//!
//! # Built-in policies
//!
//! | Policy | Behaviour |
//! |--------|-----------|
//! | [`Unrestricted`] | No checks (default) |
//! | [`Sandboxed`] | Capability-based sandbox via [`cap_std`] |
//!
//! # Security architecture
//!
//! The sandbox uses a **two-layer** design:
//!
//! 1. **Routing layer** (`normalize_for_matching`) — best-effort path
//!    resolution to select the correct `Dir` handle.  This layer resolves
//!    platform symlinks (e.g. `/tmp` → `/private/tmp` on macOS) but is
//!    **not** the security boundary.
//!
//! 2. **Enforcement layer** ([`cap_std`]) — all actual I/O goes through
//!    `cap_std::fs::Dir`, which uses `openat2` + `RESOLVE_BENEATH` on
//!    Linux 5.6+ and manual per-component resolution on other platforms.
//!    This prevents symlink escapes, `..` traversal, and absolute-path
//!    breakout at the OS level.
//!
//! ## TOCTOU note
//!
//! There is an inherent window between `normalize_for_matching` (which
//! may call `canonicalize()`) and the subsequent `cap_std` I/O.  A
//! symlink replaced in that window cannot escape the sandbox because
//! `cap_std` re-validates the path at I/O time, but it may cause
//! unexpected errors or access a different file within the same sandbox.
//!
//! ## Encoding — UTF-8 only (by design)
//!
//! All path arguments are received as Rust [`String`] (UTF-8).
//! Non-UTF-8 Lua strings are rejected at the `FromLua` boundary.
//! Returned paths use [`to_string_lossy`](std::path::Path::to_string_lossy),
//! replacing any non-UTF-8 bytes with U+FFFD.
//!
//! Raw byte (`OsStr`) round-tripping is intentionally unsupported —
//! see crate-level docs for rationale.
//! Ref: <https://docs.rs/mlua/latest/mlua/struct.String.html>

mod env_policy;
mod http;
mod llm_policy;
#[cfg(feature = "sandbox")]
mod sandbox;

pub use env_policy::*;
pub use http::*;
pub use llm_policy::*;
#[cfg(feature = "sandbox")]
pub use sandbox::Sandboxed;

use std::io;
#[cfg(any(feature = "fs", feature = "hash"))]
use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(feature = "sandbox")]
use std::sync::Arc;

/// Filesystem operation kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathOp {
    Read,
    Write,
    Delete,
    List,
}

impl std::fmt::Display for PathOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathOp::Read => f.write_str("read"),
            PathOp::Write => f.write_str("write"),
            PathOp::Delete => f.write_str("delete"),
            PathOp::List => f.write_str("list"),
        }
    }
}

// ─── PolicyError ─────────────────────────

/// Error type returned by policy `check` / `resolve` methods.
///
/// Wraps a human-readable denial reason.  Implements [`std::error::Error`]
/// so it composes naturally with `mlua::LuaError::external`.
#[derive(Debug, Clone)]
pub struct PolicyError(String);

impl PolicyError {
    /// Create a new policy error from a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    /// The denial reason.
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PolicyError {}

impl From<String> for PolicyError {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for PolicyError {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

// ─── FsAccess ────────────────────────────

/// Opaque handle to a policy-resolved filesystem path.
///
/// Returned by [`PathPolicy::resolve`].  All I/O MUST go through
/// the methods on this type — never convert back to a raw path and
/// call `std::fs` directly.
///
/// For custom [`PathPolicy`] implementations, construct with
/// [`FsAccess::direct`].
pub struct FsAccess(pub(crate) FsAccessInner);

impl std::fmt::Debug for FsAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            FsAccessInner::Direct(p) => f.debug_tuple("FsAccess::Direct").field(p).finish(),
            #[cfg(feature = "sandbox")]
            FsAccessInner::Capped { relative, .. } => {
                f.debug_tuple("FsAccess::Capped").field(relative).finish()
            }
        }
    }
}

pub(crate) enum FsAccessInner {
    /// No sandbox — delegates to `std::fs`.
    Direct(PathBuf),
    /// Capability-based sandbox via `cap_std::fs::Dir`.
    #[cfg(feature = "sandbox")]
    Capped {
        dir: Arc<cap_std::fs::Dir>,
        relative: PathBuf,
    },
}

impl FsAccess {
    /// Create a direct (unsandboxed) filesystem access handle.
    pub fn direct(path: impl Into<PathBuf>) -> Self {
        Self(FsAccessInner::Direct(path.into()))
    }

    // ── I/O operations (crate-internal) ──────────────

    pub(crate) fn file_size(&self) -> io::Result<u64> {
        match &self.0 {
            FsAccessInner::Direct(p) => Ok(std::fs::metadata(p)?.len()),
            #[cfg(feature = "sandbox")]
            FsAccessInner::Capped { dir, relative } => Ok(dir.metadata(relative)?.len()),
        }
    }

    pub(crate) fn read_to_string(&self) -> io::Result<String> {
        match &self.0 {
            FsAccessInner::Direct(p) => std::fs::read_to_string(p),
            #[cfg(feature = "sandbox")]
            FsAccessInner::Capped { dir, relative } => dir.read_to_string(relative),
        }
    }

    /// Read the file as raw bytes.
    ///
    /// Available when the `fs` feature is enabled.
    #[cfg(feature = "fs")]
    pub(crate) fn read_bytes(&self) -> io::Result<Vec<u8>> {
        match &self.0 {
            FsAccessInner::Direct(p) => std::fs::read(p),
            #[cfg(feature = "sandbox")]
            FsAccessInner::Capped { dir, relative } => dir.read(relative),
        }
    }

    pub(crate) fn write(&self, content: impl AsRef<[u8]>) -> io::Result<()> {
        match &self.0 {
            FsAccessInner::Direct(p) => std::fs::write(p, content),
            #[cfg(feature = "sandbox")]
            FsAccessInner::Capped { dir, relative } => dir.write(relative, content),
        }
    }

    #[cfg(any(feature = "fs", test))]
    pub(crate) fn exists(&self) -> bool {
        match &self.0 {
            FsAccessInner::Direct(p) => p.exists(),
            #[cfg(feature = "sandbox")]
            FsAccessInner::Capped { dir, relative } => dir.exists(relative),
        }
    }

    #[cfg(any(feature = "fs", test))]
    pub(crate) fn is_dir(&self) -> bool {
        match &self.0 {
            FsAccessInner::Direct(p) => p.is_dir(),
            #[cfg(feature = "sandbox")]
            FsAccessInner::Capped { dir, relative } => {
                dir.metadata(relative).map(|m| m.is_dir()).unwrap_or(false)
            }
        }
    }

    /// Check if the path is a regular file.
    ///
    /// Available when the `fs` feature is enabled.
    #[cfg(feature = "fs")]
    pub(crate) fn is_file(&self) -> bool {
        match &self.0 {
            FsAccessInner::Direct(p) => p.is_file(),
            #[cfg(feature = "sandbox")]
            FsAccessInner::Capped { dir, relative } => {
                dir.metadata(relative).map(|m| m.is_file()).unwrap_or(false)
            }
        }
    }

    /// Create the directory and all parent directories.
    ///
    /// Available when the `fs` feature is enabled.
    #[cfg(feature = "fs")]
    pub(crate) fn create_dir_all(&self) -> io::Result<()> {
        match &self.0 {
            FsAccessInner::Direct(p) => std::fs::create_dir_all(p),
            #[cfg(feature = "sandbox")]
            FsAccessInner::Capped { dir, relative } => dir.create_dir_all(relative),
        }
    }

    #[cfg(any(feature = "fs", test))]
    /// Remove a file or directory.
    ///
    /// Tries `remove_file` first. On failure, falls back to `remove_dir_all`
    /// only when the error indicates the target is a directory:
    ///
    /// - Linux: `unlink()` returns `EISDIR` → `ErrorKind::IsADirectory`
    /// - macOS/BSD: `unlink()` returns `EPERM` → `ErrorKind::PermissionDenied`
    ///
    /// On macOS, `PermissionDenied` is ambiguous (could be a genuine
    /// permission error on a file).  If `remove_dir_all` also fails
    /// (e.g. target was a file, not a directory), the **original**
    /// `remove_file` error is returned so diagnostics remain accurate.
    /// All other error kinds are propagated immediately.
    pub(crate) fn remove(&self) -> io::Result<()> {
        match &self.0 {
            FsAccessInner::Direct(p) => match std::fs::remove_file(p) {
                Ok(()) => Ok(()),
                Err(e) if is_unlink_dir_error(&e) => std::fs::remove_dir_all(p).map_err(|_| e),
                Err(e) => Err(e),
            },
            #[cfg(feature = "sandbox")]
            FsAccessInner::Capped { dir, relative } => match dir.remove_file(relative) {
                Ok(()) => Ok(()),
                Err(e) if is_unlink_dir_error(&e) => dir.remove_dir_all(relative).map_err(|_| e),
                Err(e) => Err(e),
            },
        }
    }

    /// Open the file for buffered reading.
    ///
    /// Available when the `fs` or `hash` feature is enabled.
    #[cfg(any(feature = "fs", feature = "hash"))]
    pub(crate) fn open_read(&self) -> io::Result<Box<dyn Read>> {
        match &self.0 {
            FsAccessInner::Direct(p) => {
                let f = std::fs::File::open(p)?;
                Ok(Box::new(io::BufReader::new(f)))
            }
            #[cfg(feature = "sandbox")]
            FsAccessInner::Capped { dir, relative } => {
                let f = dir.open(relative)?;
                Ok(Box::new(io::BufReader::new(f)))
            }
        }
    }

    pub(crate) fn canonicalize(&self) -> io::Result<PathBuf> {
        match &self.0 {
            FsAccessInner::Direct(p) => p.canonicalize(),
            #[cfg(feature = "sandbox")]
            FsAccessInner::Capped { .. } => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "canonicalize is not available in sandboxed mode",
            )),
        }
    }

    /// Walk this path recursively, collecting file paths that pass `filter`.
    ///
    /// `display_prefix` is the user-visible path prefix to prepend
    /// (e.g. the original dir_path the user passed to `fs.walk`).
    #[cfg(feature = "fs")]
    pub(crate) fn walk_files_filtered(
        &self,
        display_prefix: &Path,
        filter: &dyn Fn(&str) -> bool,
        max_depth: usize,
        max_entries: usize,
    ) -> io::Result<Vec<String>> {
        let mut results = Vec::new();
        match &self.0 {
            FsAccessInner::Direct(p) => {
                for entry in walkdir::WalkDir::new(p).max_depth(max_depth) {
                    match entry {
                        Ok(e) if e.file_type().is_file() => {
                            let path_str = e.path().to_string_lossy();
                            if filter(&path_str) {
                                if results.len() >= max_entries {
                                    return Err(io::Error::other(format!(
                                        "entry limit exceeded ({max_entries})"
                                    )));
                                }
                                results.push(path_str.into_owned());
                            }
                        }
                        Ok(_) => {}
                        Err(e) => return Err(e.into()),
                    }
                }
            }
            #[cfg(feature = "sandbox")]
            FsAccessInner::Capped { dir, relative } => {
                let walk_root = dir.open_dir(relative)?;
                sandbox::walk_capped_filtered(
                    &walk_root,
                    display_prefix,
                    filter,
                    0,
                    max_depth,
                    max_entries,
                    &mut results,
                )?;
            }
        }
        Ok(results)
    }

    /// Walk this path recursively, collecting all file paths.
    #[cfg(feature = "fs")]
    pub(crate) fn walk_files(
        &self,
        display_prefix: &Path,
        max_depth: usize,
        max_entries: usize,
    ) -> io::Result<Vec<String>> {
        self.walk_files_filtered(display_prefix, &|_| true, max_depth, max_entries)
    }

    /// Copy this file's contents to `dst`.
    ///
    /// Available when the `fs` feature is enabled.
    #[cfg(feature = "fs")]
    pub(crate) fn copy_to(&self, dst: &FsAccess) -> io::Result<u64> {
        match (&self.0, &dst.0) {
            (FsAccessInner::Direct(src), FsAccessInner::Direct(d)) => std::fs::copy(src, d),
            #[cfg(feature = "sandbox")]
            _ => {
                let content = self.read_bytes()?;
                // content.len() fits in u64 (Vec max is isize::MAX < u64::MAX).
                let len = content.len() as u64;
                dst.write(&content)?;
                Ok(len)
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn display(&self) -> String {
        match &self.0 {
            FsAccessInner::Direct(p) => p.to_string_lossy().to_string(),
            #[cfg(feature = "sandbox")]
            FsAccessInner::Capped { relative, .. } => relative.to_string_lossy().to_string(),
        }
    }
}

/// Check if a `remove_file` error indicates the target *may* be a directory.
///
/// Platform behaviour of `unlink()` on a directory:
/// - Linux: `EISDIR` → `ErrorKind::IsADirectory`
/// - macOS / BSD: `EPERM` → `ErrorKind::PermissionDenied`
///   (POSIX specifies `EPERM` for directory unlink)
///
/// On macOS, `PermissionDenied` is ambiguous: it could be a genuine
/// permission error on a file.  Callers handle this by attempting
/// `remove_dir_all` and falling back to the original error on failure.
#[cfg(any(feature = "fs", test))]
fn is_unlink_dir_error(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::IsADirectory | io::ErrorKind::PermissionDenied
    )
}

// ─── PathPolicy trait ────────────────────────────

/// Policy that decides whether a given path may be accessed.
///
/// Every filesystem-touching function in `mlua-batteries` calls
/// [`PathPolicy::resolve`] before performing I/O.
pub trait PathPolicy: Send + Sync + 'static {
    /// Human-readable name for this policy, used in `Debug` output.
    ///
    /// The default implementation returns [`std::any::type_name`] of the
    /// concrete type, which works correctly even through trait objects
    /// because the vtable dispatches to the concrete implementation.
    fn policy_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    /// Validate `path` for `op` and return an [`FsAccess`] handle.
    ///
    /// Return `Ok(handle)` to allow, `Err(reason)` to deny.
    fn resolve(&self, path: &Path, op: PathOp) -> Result<FsAccess, PolicyError>;
}

// ─── Unrestricted ────────────────────────────

/// No restrictions — every path is allowed as-is.
///
/// This is the default policy used by [`crate::register_all`].
///
/// # Warning
///
/// With this policy, Lua scripts can read, write, and delete **any** file
/// accessible to the process.  Do **not** use this policy with untrusted
/// scripts.  Use [`Sandboxed`] instead.
#[derive(Debug)]
pub struct Unrestricted;

impl PathPolicy for Unrestricted {
    fn resolve(&self, path: &Path, _op: PathOp) -> Result<FsAccess, PolicyError> {
        Ok(FsAccess::direct(path))
    }
}

#[cfg(test)]
mod tests;
