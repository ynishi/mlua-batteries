//! Capability-based sandbox using `cap_std`.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{FsAccess, FsAccessInner, PathOp, PathPolicy, PolicyError};

/// Capability-based sandbox restricting access to allowed root directories.
///
/// Each root is opened as a [`cap_std::fs::Dir`] at construction time.
/// All subsequent I/O goes through the `Dir` handle, which prevents
/// path traversal (symlinks, `..`) at the OS level.
///
/// On Linux 5.6+ this uses `openat2` with `RESOLVE_BENEATH`.
/// On other platforms, `cap-std` manually resolves each path component.
///
/// ```rust,no_run
/// use mlua_batteries::policy::Sandboxed;
///
/// let policy = Sandboxed::new(["/app/data", "/tmp"]).unwrap()
///     .read_only();
/// ```
pub struct Sandboxed {
    /// `(canonical_root_path, Dir_handle)` pairs.
    roots: Vec<(PathBuf, Arc<cap_std::fs::Dir>)>,
    read_only: bool,
}

impl std::fmt::Debug for Sandboxed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sandboxed")
            .field(
                "roots",
                &self.roots.iter().map(|(p, _)| p).collect::<Vec<_>>(),
            )
            .field("read_only", &self.read_only)
            .finish()
    }
}

impl Sandboxed {
    /// Create a sandbox allowing access under the given root directories.
    ///
    /// Each root is canonicalized and opened as a `Dir` immediately,
    /// so the directories must exist at the time of construction.
    pub fn new<I, P>(roots: I) -> io::Result<Self>
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        let mut root_pairs = Vec::new();
        for root in roots {
            let path: PathBuf = root.into();
            let canonical = path.canonicalize()?;
            let dir = cap_std::fs::Dir::open_ambient_dir(&canonical, cap_std::ambient_authority())?;
            root_pairs.push((canonical, Arc::new(dir)));
        }
        if root_pairs.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Sandboxed requires at least one root directory",
            ));
        }
        Ok(Self {
            roots: root_pairs,
            read_only: false,
        })
    }

    /// Deny write and delete operations.
    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }
}

impl PathPolicy for Sandboxed {
    fn resolve(&self, path: &Path, op: PathOp) -> Result<FsAccess, PolicyError> {
        if self.read_only && matches!(op, PathOp::Write | PathOp::Delete) {
            return Err(PolicyError::new(format!(
                "{op} denied: filesystem is read-only (path '{}')",
                path.display()
            )));
        }

        let normalized = normalize_for_matching(path)?;

        for (root_canonical, dir) in &self.roots {
            if let Ok(relative) = normalized.strip_prefix(root_canonical) {
                let relative = if relative.as_os_str().is_empty() {
                    PathBuf::from(".")
                } else {
                    relative.to_path_buf()
                };
                return Ok(FsAccess(FsAccessInner::Capped {
                    dir: Arc::clone(dir),
                    relative,
                }));
            }
        }

        Err(PolicyError::new(format!(
            "{op} denied: path '{}' is outside allowed directories",
            path.display()
        )))
    }
}

/// Normalize a user-provided path for root matching.
///
/// This function is for **routing only** — to determine which `Dir` handle
/// to use. The actual security enforcement is done by `cap_std` at I/O time
/// via `openat2` / `RESOLVE_BENEATH`.
///
/// # Symlink resolution
///
/// Platform symlinks (e.g. macOS `/tmp` → `/private/tmp`) must be resolved
/// to match the canonicalized sandbox roots.  When the full path doesn't
/// exist yet, we walk ancestors upward until we find one that can be
/// canonicalized, then append the remaining tail.  This handles deeply
/// nested new paths under symlinked directories.
///
/// # Strategy
///
/// 1. Make path absolute (join with CWD if relative)
/// 2. Lexically clean (resolve `.` / `..` without FS access)
/// 3. Try `canonicalize()` on the full path
/// 4. Walk ancestors upward: canonicalize the deepest existing ancestor,
///    append remaining components
/// 5. Fallback: return the lexically cleaned path (zero FS access)
fn normalize_for_matching(path: &Path) -> Result<PathBuf, PolicyError> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| PolicyError::new(format!("cannot determine working directory: {e}")))?
            .join(path)
    };

    let cleaned = lexical_clean(&abs);

    // Prefer full canonicalization (resolves symlinks).
    if let Ok(c) = cleaned.canonicalize() {
        return Ok(c);
    }

    // Path doesn't exist yet — walk ancestors upward to find the deepest
    // existing ancestor and canonicalize it.  This correctly resolves
    // platform symlinks (e.g. /tmp → /private/tmp on macOS) even for
    // paths like /tmp/sandbox/new/deep/file.txt where only /tmp exists.
    let mut tail = PathBuf::new();
    let mut ancestor = cleaned.as_path();
    loop {
        match ancestor.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => {
                // Prepend the current leaf to the tail
                if let Some(name) = ancestor.file_name() {
                    tail = Path::new(name).join(&tail);
                }
                ancestor = parent;
                if let Ok(canonical_ancestor) = ancestor.canonicalize() {
                    let result = if tail.as_os_str().is_empty() {
                        canonical_ancestor
                    } else {
                        canonical_ancestor.join(&tail)
                    };
                    return Ok(result);
                }
            }
            _ => break,
        }
    }

    // Pure lexical fallback — no ancestor could be canonicalized.
    Ok(cleaned)
}

/// Lexically normalize a path by resolving `.` and `..` components.
///
/// Does **not** access the filesystem — symlinks are **not** resolved.
/// Used only for UX-level path cleaning; security enforcement is
/// done by `cap_std`.
pub(crate) fn lexical_clean(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    let mut has_root = false;

    for c in path.components() {
        match c {
            Component::Prefix(_) => {
                parts.clear();
                parts.push(c.as_os_str().to_os_string());
            }
            Component::RootDir => {
                has_root = true;
                parts.clear();
            }
            Component::CurDir => {} // skip "."
            Component::ParentDir => {
                if !parts.is_empty()
                    && !parts.last().is_some_and(|p| {
                        Path::new(p).components().next() == Some(Component::ParentDir)
                    })
                {
                    parts.pop();
                } else if !has_root {
                    // Relative path: keep ".." when no normal component to pop
                    parts.push("..".into());
                }
                // Absolute path at root: silently drop ".."
            }
            Component::Normal(s) => {
                parts.push(s.to_os_string());
            }
        }
    }

    let mut result = PathBuf::new();
    if has_root {
        result.push("/");
    }
    for part in &parts {
        result.push(part);
    }
    if result.as_os_str().is_empty() {
        result.push(".");
    }
    result
}

/// Walk a `cap_std::fs::Dir` recursively, collecting file paths that
/// pass the `filter` predicate.
///
/// Used by `fs.glob` in sandboxed mode (with a globset matcher as filter)
/// and by `fs.walk` (with an always-true filter via [`walk_capped`]).
///
/// # Stack usage
///
/// Uses OS-thread recursion bounded by `max_depth`. Each frame is small
/// (~200 bytes), so the default `max_depth=256` uses ~50 KiB of stack,
/// well within the default 8 MiB thread stack. Extremely deep trees
/// combined with reduced stack sizes could overflow; `max_depth` guards
/// against this.
#[cfg(feature = "fs")]
pub(crate) fn walk_capped_filtered(
    dir: &cap_std::fs::Dir,
    prefix: &Path,
    filter: &dyn Fn(&str) -> bool,
    depth: usize,
    max_depth: usize,
    max_entries: usize,
    results: &mut Vec<String>,
) -> io::Result<()> {
    if depth > max_depth {
        return Ok(());
    }
    for entry in dir.read_dir(".")? {
        let entry = entry?;
        let name = entry.file_name();
        let path = prefix.join(&name);
        let ft = entry.file_type()?;
        if ft.is_file() {
            let path_str = path.to_string_lossy();
            if filter(&path_str) {
                if results.len() >= max_entries {
                    return Err(io::Error::other(format!(
                        "entry limit exceeded ({max_entries})"
                    )));
                }
                results.push(path_str.into_owned());
            }
        } else if ft.is_dir() {
            let sub = dir.open_dir(Path::new(&name))?;
            walk_capped_filtered(
                &sub,
                &path,
                filter,
                depth + 1,
                max_depth,
                max_entries,
                results,
            )?;
        }
    }
    Ok(())
}
