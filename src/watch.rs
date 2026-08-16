//! Filesystem versioning watcher — `std.watch`.
//!
//! Ported from mlua-mcp-server's guard-mcp v0 watcher. A dedicated Rust
//! thread watches a directory tree recursively (`notify` crate) and
//! records every content-changing event into a content-addressed blob
//! store plus an append-only journal:
//!
//! ```text
//! <base>/store/objects/<sha256>   -- blob, deduplicated by content hash
//! <base>/journal.jsonl            -- {ts, path, event, sha, size} per line
//! ```
//!
//! `<base>` defaults to `~/.local/share/guard-mcp` (guard-mcp compatible
//! store layout) and can be overridden per call with `opts.store_dir`.
//!
//! Lua surface:
//!
//! ```lua
//! -- start watching (returns a handle userdata; the watch stops when the
//! -- handle is garbage-collected or handle:stop() is called — keep it
//! -- referenced, e.g. in a global, for the process lifetime)
//! local handle = std.watch.start("/path/to/root", { store_dir = "/base" })
//!
//! -- version listing (exact journaled path or path suffix, oldest first)
//! local versions = std.watch.history("/path/or/suffix", { store_dir = "/base" })
//! -- versions = { { ts=<epoch>, event="modify", sha="...", size=N, path="..." }, ... }
//!
//! -- restore a recorded blob to a destination path
//! local r = std.watch.restore("/dest/path", "<sha256>", { store_dir = "/base" })
//! -- r = { restored = "/dest/path", sha = "<sha256>", size = N }
//! ```
//!
//! Properties:
//! - **all versions** are kept at write granularity (not periodic
//!   snapshots); Edit/Write-tool changes, external-editor changes and
//!   exec-driven changes are all captured equally
//! - deletions are journaled (`event = "delete"`, `sha = null`) so the
//!   pre-delete blob remains restorable
//! - identical content dedupes to a single blob
//! - **journal dedup (bug fix over the original port source)**: `notify`
//!   emits several events for one logical write (`create` + `modify` +
//!   `close_write`, or multiple `Modify` events); the watcher remembers
//!   the last journaled sha per path and skips appending a journal line
//!   when the content sha is unchanged. A `delete` (or an oversized
//!   skip) resets that memory, so re-creating a file with identical
//!   content is journaled again
//! - excluded directories ([`DEFAULT_EXCLUDE_DIRS`]) and files larger
//!   than [`MAX_FILE_SIZE_BYTES`] are skipped; an oversized file is
//!   journaled as `event = "skip_too_large"`
//! - on event-queue overflow (`need_rescan` flag / watch errors) a
//!   `event = "gap"` marker line is journaled so history consumers can
//!   see the blind spot
//!
//! **Policy note**: this module intentionally does **not** route file
//! access through [`crate::policy`] — the versioning store commonly
//! lives outside a sandbox root (e.g. `~/.local/share/guard-mcp` while
//! the sandbox is the project tree), and `restore` must be able to write
//! back into the watched tree. Hosts that need a boundary should gate
//! the Lua callers instead (guard-mcp does this at its tool layer).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use mlua::prelude::*;
use notify::event::{AccessKind, AccessMode, EventKind};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use sha2::{Digest, Sha256};

/// Directories excluded from watching, matched per path component
/// relative to the watch root.
pub const DEFAULT_EXCLUDE_DIRS: [&str; 4] = [".git", "target", "node_modules", ".worktrees"];

/// Files above this size are not stored; a `skip_too_large` journal
/// entry is written instead.
pub const MAX_FILE_SIZE_BYTES: u64 = 50 * 1024 * 1024;

// ─── Config / handle / state ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct WatcherConfig {
    /// Tree being watched.
    pub root: PathBuf,
    /// Store base dir (contains `store/objects/` and `journal.jsonl`).
    pub base_dir: PathBuf,
    /// Excluded directory names (per path component under `root`).
    pub excludes: Vec<String>,
    /// Per-file size cap.
    pub max_file_size: u64,
}

impl WatcherConfig {
    pub fn objects_dir(&self) -> PathBuf {
        self.base_dir.join("store").join("objects")
    }

    pub fn journal_path(&self) -> PathBuf {
        self.base_dir.join("journal.jsonl")
    }
}

/// Per-path journal dedup memory (the journal-duplication bug fix).
///
/// `notify` fires several events for one logical write; recording each
/// would fill the journal with identical consecutive lines. The event
/// thread keeps the last journaled content sha per path and skips
/// appends whose sha is unchanged. Deletions and oversized skips clear
/// the entry so genuinely new history is never suppressed.
#[derive(Debug, Default)]
pub struct DedupState {
    last_sha: HashMap<PathBuf, String>,
}

/// Keeps the watcher (and its event thread) alive. Dropping the handle
/// stops the watch and lets the thread terminate.
pub struct WatcherHandle {
    _watcher: RecommendedWatcher,
    _thread: std::thread::JoinHandle<()>,
}

/// Resolve the default store base dir: `~/.local/share/guard-mcp`
/// (guard-mcp compatible layout). Callers override per call via
/// `opts.store_dir`.
pub fn default_base_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| "HOME not set and no store_dir provided".to_string())?;
    Ok(PathBuf::from(home).join(".local/share/guard-mcp"))
}

// ─── Watcher thread ──────────────────────────────────────────────────────

/// Start a watcher thread for `config`. The returned handle must be kept
/// alive for as long as the watch should run.
pub fn spawn_watcher(config: WatcherConfig) -> Result<WatcherHandle, String> {
    std::fs::create_dir_all(config.objects_dir())
        .map_err(|e| format!("create store dir {}: {e}", config.objects_dir().display()))?;

    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<Event>>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        let _ = tx.send(res);
    })
    .map_err(|e| format!("create watcher: {e}"))?;
    watcher
        .watch(&config.root, RecursiveMode::Recursive)
        .map_err(|e| format!("watch {}: {e}", config.root.display()))?;
    tracing::info!(
        "std.watch started: root={} store={}",
        config.root.display(),
        config.base_dir.display()
    );

    let thread = std::thread::spawn(move || {
        let mut state = DedupState::default();
        while let Ok(res) = rx.recv() {
            match res {
                Ok(event) => handle_event(&config, &mut state, &event),
                Err(e) => {
                    // Watch-level error (includes queue overflow on some
                    // backends): journal a gap marker so history readers
                    // know events may be missing.
                    let _ = append_gap_marker(&config, &format!("watch error: {e}"));
                }
            }
        }
    });

    Ok(WatcherHandle {
        _watcher: watcher,
        _thread: thread,
    })
}

// ─── Event handling ──────────────────────────────────────────────────────

fn handle_event(config: &WatcherConfig, state: &mut DedupState, event: &Event) {
    if event.need_rescan() {
        // inotify queue overflow: events were dropped by the kernel.
        let _ = append_gap_marker(config, "rescan flagged (event queue overflow)");
    }
    let label = match &event.kind {
        EventKind::Remove(_) => "delete",
        EventKind::Create(_) => "create",
        EventKind::Modify(_) => "modify",
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => "close_write",
        _ => return,
    };
    for path in &event.paths {
        if is_excluded(&config.root, path, &config.excludes) {
            continue;
        }
        let r = if label == "delete" {
            record_delete(config, state, path)
        } else {
            record_snapshot(config, state, path, label)
        };
        if let Err(e) = r {
            tracing::warn!("std.watch: record {} {}: {e}", label, path.display());
        }
    }
}

/// A path is excluded when it is outside the root or any of its
/// components (relative to root) matches an exclude entry.
pub fn is_excluded(root: &Path, path: &Path, excludes: &[String]) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return true;
    };
    rel.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        excludes.iter().any(|e| e.trim_end_matches('/') == s)
    })
}

/// Snapshot `path` into the store and journal the event. Missing files
/// (raced deletion / rename source) are silently skipped; directories
/// are ignored. Consecutive events whose content sha matches the last
/// journaled sha for the same path are skipped (journal dedup fix).
pub fn record_snapshot(
    config: &WatcherConfig,
    state: &mut DedupState,
    path: &Path,
    event: &str,
) -> Result<(), String> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Ok(()); // vanished between event and read — not an error
    };
    if !meta.is_file() {
        return Ok(());
    }
    if meta.len() > config.max_file_size {
        // Reset dedup memory: after a skip window the next in-limit
        // content must be journaled even if it matches the pre-skip sha.
        state.last_sha.remove(path);
        return append_journal_line(
            config,
            &serde_json::json!({
                "ts": now_epoch(),
                "path": path.to_string_lossy(),
                "event": "skip_too_large",
                "sha": serde_json::Value::Null,
                "size": meta.len(),
            }),
        );
    }
    let Ok(content) = std::fs::read(path) else {
        return Ok(()); // raced deletion
    };
    let sha = hex_sha256(&content);

    // Journal dedup fix: notify emits several events per logical write
    // (create + modify + close_write / repeated Modify). Skip the append
    // when this path's content sha has not changed since the last
    // journaled line.
    if state.last_sha.get(path).is_some_and(|last| *last == sha) {
        return Ok(());
    }

    let blob_path = config.objects_dir().join(&sha);
    if !blob_path.exists() {
        // Content-addressed: concurrent writers produce identical bytes,
        // so a plain write is collision-safe.
        std::fs::write(&blob_path, &content)
            .map_err(|e| format!("write blob {}: {e}", blob_path.display()))?;
    }
    append_journal_line(
        config,
        &serde_json::json!({
            "ts": now_epoch(),
            "path": path.to_string_lossy(),
            "event": event,
            "sha": sha,
            "size": content.len(),
        }),
    )?;
    state.last_sha.insert(path.to_path_buf(), sha);
    Ok(())
}

/// Journal a deletion (no blob write; the pre-delete blob stays in the
/// store). Clears the dedup memory for the path so a later re-create
/// with identical content is journaled again.
pub fn record_delete(
    config: &WatcherConfig,
    state: &mut DedupState,
    path: &Path,
) -> Result<(), String> {
    state.last_sha.remove(path);
    append_journal_line(
        config,
        &serde_json::json!({
            "ts": now_epoch(),
            "path": path.to_string_lossy(),
            "event": "delete",
            "sha": serde_json::Value::Null,
            "size": serde_json::Value::Null,
        }),
    )
}

/// Journal a gap marker (event loss window).
pub fn append_gap_marker(config: &WatcherConfig, detail: &str) -> Result<(), String> {
    append_journal_line(
        config,
        &serde_json::json!({
            "ts": now_epoch(),
            "event": "gap",
            "detail": detail,
        }),
    )
}

fn append_journal_line(config: &WatcherConfig, value: &serde_json::Value) -> Result<(), String> {
    use std::io::Write;
    let path = config.journal_path();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("open journal {}: {e}", path.display()))?;
    writeln!(f, "{value}").map_err(|e| format!("append journal: {e}"))?;
    Ok(())
}

fn now_epoch() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

// ─── History / restore (journal + store readers) ─────────────────────────

/// One journaled version entry as returned by [`history_entries`].
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub ts: Option<f64>,
    pub event: Option<String>,
    pub sha: Option<String>,
    pub size: Option<u64>,
    pub path: String,
}

/// List journal entries whose path equals `query` or ends with it
/// (suffix match), oldest first. A missing journal yields an empty list.
pub fn history_entries(base_dir: &Path, query: &str) -> Vec<HistoryEntry> {
    let Ok(text) = std::fs::read_to_string(base_dir.join("journal.jsonl")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(path) = entry.get("path").and_then(|p| p.as_str()) else {
            continue; // gap markers carry no path
        };
        if path == query || path.ends_with(query) {
            out.push(HistoryEntry {
                ts: entry.get("ts").and_then(|v| v.as_f64()),
                event: entry
                    .get("event")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                sha: entry
                    .get("sha")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                size: entry.get("size").and_then(|v| v.as_u64()),
                path: path.to_string(),
            });
        }
    }
    out
}

/// Restore the content-addressed blob `sha` to `dest`. Returns the
/// restored byte count. The sha must be lowercase hex; missing blobs and
/// write failures are errors.
pub fn restore_blob(base_dir: &Path, dest: &Path, sha: &str) -> Result<usize, String> {
    if sha.is_empty() || sha.bytes().any(|b| !matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(format!("invalid sha (lowercase hex expected): {sha}"));
    }
    let blob_path = base_dir.join("store").join("objects").join(sha);
    let content = std::fs::read(&blob_path)
        .map_err(|_| format!("blob not found: {sha} ({})", blob_path.display()))?;
    std::fs::write(dest, &content)
        .map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
    Ok(content.len())
}

// ─── Lua binding ─────────────────────────────────────────────────────────

/// Lua-visible watch handle. Keeps the watcher thread alive; `stop()`
/// (or garbage collection) drops the underlying watch.
pub struct LuaWatchHandle {
    inner: Option<WatcherHandle>,
}

impl LuaUserData for LuaWatchHandle {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut("stop", |_, this, ()| {
            this.inner = None;
            Ok(())
        });
        methods.add_method("running", |_, this, ()| Ok(this.inner.is_some()));
    }
}

/// Resolve `opts.store_dir` (or the guard-mcp compatible default) from
/// an optional Lua opts table.
fn resolve_base_dir(opts: &Option<LuaTable>) -> LuaResult<PathBuf> {
    if let Some(o) = opts {
        if let Ok(s) = o.get::<String>("store_dir") {
            return Ok(PathBuf::from(s));
        }
    }
    default_base_dir().map_err(LuaError::external)
}

/// Module entry point: builds the `watch` table
/// (`watch.start` / `watch.history` / `watch.restore`).
pub fn module(lua: &Lua) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;

    t.set(
        "start",
        lua.create_function(|_, (root, opts): (String, Option<LuaTable>)| {
            let root_path = PathBuf::from(&root);
            if !root_path.is_dir() {
                // Fail-closed: refuse to "watch" an unresolvable root.
                return Err(LuaError::external(format!(
                    "watch.start: {root} is not an existing directory"
                )));
            }
            let base_dir = resolve_base_dir(&opts)?;
            let config = WatcherConfig {
                root: root_path,
                base_dir,
                excludes: DEFAULT_EXCLUDE_DIRS.iter().map(|s| s.to_string()).collect(),
                max_file_size: MAX_FILE_SIZE_BYTES,
            };
            let handle = spawn_watcher(config).map_err(LuaError::external)?;
            Ok(LuaWatchHandle {
                inner: Some(handle),
            })
        })?,
    )?;

    t.set(
        "history",
        lua.create_function(|lua, (path, opts): (String, Option<LuaTable>)| {
            let base = resolve_base_dir(&opts)?;
            let entries = history_entries(&base, &path);
            let out = lua.create_table()?;
            for (i, e) in entries.iter().enumerate() {
                let row = lua.create_table()?;
                match e.ts {
                    Some(ts) => row.set("ts", ts)?,
                    None => row.set("ts", LuaValue::Nil)?,
                }
                row.set("event", e.event.as_deref())?;
                row.set("sha", e.sha.as_deref())?;
                match e.size {
                    Some(s) => row.set("size", s)?,
                    None => row.set("size", LuaValue::Nil)?,
                }
                row.set("path", e.path.as_str())?;
                out.set(i + 1, row)?;
            }
            Ok(out)
        })?,
    )?;

    t.set(
        "restore",
        lua.create_function(|lua, (path, sha, opts): (String, String, Option<LuaTable>)| {
            let base = resolve_base_dir(&opts)?;
            let size =
                restore_blob(&base, Path::new(&path), &sha).map_err(LuaError::external)?;
            let out = lua.create_table()?;
            out.set("restored", path)?;
            out.set("sha", sha)?;
            out.set("size", size)?;
            Ok(out)
        })?,
    )?;

    Ok(t)
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn unique_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "mlua_bat_watch_{}_{}_{name}",
            std::process::id(),
            nanos
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_config(root: PathBuf, base: PathBuf) -> WatcherConfig {
        let config = WatcherConfig {
            root,
            base_dir: base,
            excludes: DEFAULT_EXCLUDE_DIRS.iter().map(|s| s.to_string()).collect(),
            max_file_size: MAX_FILE_SIZE_BYTES,
        };
        std::fs::create_dir_all(config.objects_dir()).unwrap();
        config
    }

    fn journal_lines(config: &WatcherConfig) -> Vec<serde_json::Value> {
        let Ok(text) = std::fs::read_to_string(config.journal_path()) else {
            return Vec::new();
        };
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn blob_count(config: &WatcherConfig) -> usize {
        std::fs::read_dir(config.objects_dir())
            .map(|it| it.count())
            .unwrap_or(0)
    }

    /// Regression test for the journal-duplication bug: notify emits
    /// several events per logical write; consecutive same-sha events for
    /// a path must journal exactly once.
    #[test]
    fn journal_dedup_skips_consecutive_same_sha_events() {
        let root = unique_dir("dedup_fix_root");
        let base = unique_dir("dedup_fix_base");
        let config = test_config(root.clone(), base.clone());
        let mut state = DedupState::default();
        let file = root.join("a.txt");
        std::fs::write(&file, "same content").unwrap();

        // One logical write surfaces as create + modify + close_write.
        record_snapshot(&config, &mut state, &file, "create").unwrap();
        record_snapshot(&config, &mut state, &file, "modify").unwrap();
        record_snapshot(&config, &mut state, &file, "close_write").unwrap();
        let lines = journal_lines(&config);
        assert_eq!(lines.len(), 1, "same-sha burst must journal once: {lines:?}");
        assert_eq!(lines[0]["event"], "create");
        assert_eq!(blob_count(&config), 1);

        // Changed content → journaled again.
        std::fs::write(&file, "different content").unwrap();
        record_snapshot(&config, &mut state, &file, "modify").unwrap();
        let lines = journal_lines(&config);
        assert_eq!(lines.len(), 2);
        assert_ne!(lines[0]["sha"], lines[1]["sha"]);
        assert_eq!(blob_count(&config), 2);

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Delete resets the dedup memory: re-creating a file with identical
    /// content must be journaled as new history.
    #[test]
    fn delete_resets_dedup_state() {
        let root = unique_dir("dedup_del_root");
        let base = unique_dir("dedup_del_base");
        let config = test_config(root.clone(), base.clone());
        let mut state = DedupState::default();
        let file = root.join("b.txt");
        std::fs::write(&file, "payload").unwrap();

        record_snapshot(&config, &mut state, &file, "create").unwrap();
        std::fs::remove_file(&file).unwrap();
        record_delete(&config, &mut state, &file).unwrap();
        std::fs::write(&file, "payload").unwrap();
        record_snapshot(&config, &mut state, &file, "create").unwrap();

        let lines = journal_lines(&config);
        assert_eq!(lines.len(), 3, "create / delete / re-create: {lines:?}");
        assert_eq!(lines[0]["event"], "create");
        assert_eq!(lines[1]["event"], "delete");
        assert_eq!(lines[2]["event"], "create");
        assert_eq!(lines[0]["sha"], lines[2]["sha"], "same content, same blob");
        assert_eq!(blob_count(&config), 1, "identical content dedupes to one blob");

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn delete_and_gap_are_journaled() {
        let root = unique_dir("delete_root");
        let base = unique_dir("delete_base");
        let config = test_config(root.clone(), base.clone());
        let mut state = DedupState::default();

        record_delete(&config, &mut state, &root.join("gone.txt")).unwrap();
        append_gap_marker(&config, "test overflow").unwrap();

        let lines = journal_lines(&config);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["event"], "delete");
        assert!(lines[0]["sha"].is_null());
        assert_eq!(lines[1]["event"], "gap");
        assert_eq!(lines[1]["detail"], "test overflow");

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn excluded_components_are_skipped() {
        let root = PathBuf::from("/w");
        let ex: Vec<String> = DEFAULT_EXCLUDE_DIRS.iter().map(|s| s.to_string()).collect();
        assert!(is_excluded(&root, Path::new("/w/.git/config"), &ex));
        assert!(is_excluded(&root, Path::new("/w/target/debug/foo"), &ex));
        assert!(is_excluded(&root, Path::new("/w/a/node_modules/b.js"), &ex));
        assert!(is_excluded(&root, Path::new("/w/.worktrees/t/x.rs"), &ex));
        assert!(is_excluded(&root, Path::new("/outside/x.rs"), &ex), "outside root");
        assert!(!is_excluded(&root, Path::new("/w/src/main.rs"), &ex));
        assert!(!is_excluded(&root, Path::new("/w/targets/x.rs"), &ex), "prefix only");
    }

    #[test]
    fn oversized_file_journals_skip_and_stores_no_blob() {
        let root = unique_dir("large_root");
        let base = unique_dir("large_base");
        let mut config = test_config(root.clone(), base.clone());
        config.max_file_size = 4;
        let mut state = DedupState::default();
        let file = root.join("big.bin");
        std::fs::write(&file, "0123456789").unwrap();

        record_snapshot(&config, &mut state, &file, "create").unwrap();
        assert_eq!(blob_count(&config), 0);
        let lines = journal_lines(&config);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["event"], "skip_too_large");
        assert_eq!(lines[0]["size"], 10);

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn history_and_restore_roundtrip() {
        let root = unique_dir("hist_root");
        let base = unique_dir("hist_base");
        let config = test_config(root.clone(), base.clone());
        let mut state = DedupState::default();
        let file = root.join("doc.md");
        std::fs::write(&file, "version one").unwrap();
        record_snapshot(&config, &mut state, &file, "create").unwrap();
        std::fs::write(&file, "version two").unwrap();
        record_snapshot(&config, &mut state, &file, "modify").unwrap();

        // Exact-path and suffix lookups both find the two versions.
        let full = history_entries(&base, &file.to_string_lossy());
        assert_eq!(full.len(), 2);
        let by_suffix = history_entries(&base, "doc.md");
        assert_eq!(by_suffix.len(), 2);
        assert_eq!(by_suffix[0].event.as_deref(), Some("create"));
        assert_eq!(by_suffix[1].event.as_deref(), Some("modify"));

        // Restore the first version and confirm the bytes round-trip.
        let sha1 = by_suffix[0].sha.clone().expect("content sha");
        let dest = root.join("restored.md");
        let size = restore_blob(&base, &dest, &sha1).unwrap();
        assert_eq!(size, "version one".len());
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "version one");

        // Invalid / unknown shas are errors.
        assert!(restore_blob(&base, &dest, "NOT-HEX").is_err());
        assert!(restore_blob(&base, &dest, &"0".repeat(64)).is_err());

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn watcher_end_to_end_records_write_and_delete() {
        let root = unique_dir("e2e_root");
        let base = unique_dir("e2e_base");
        let config = test_config(root.clone(), base.clone());
        let handle = spawn_watcher(config.clone()).unwrap();

        let file = root.join("watched.txt");
        std::fs::write(&file, "version one").unwrap();

        let wait_for = |pred: &dyn Fn(&[serde_json::Value]) -> bool| -> bool {
            for _ in 0..100 {
                if pred(&journal_lines(&config)) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            false
        };

        assert!(
            wait_for(&|lines| lines
                .iter()
                .any(|l| l["path"].as_str().is_some_and(|p| p.ends_with("watched.txt"))
                    && l["sha"].is_string())),
            "content event for watched.txt not journaled; journal = {:?}",
            journal_lines(&config)
        );

        std::fs::remove_file(&file).unwrap();
        assert!(
            wait_for(&|lines| lines
                .iter()
                .any(|l| l["event"] == "delete"
                    && l["path"].as_str().is_some_and(|p| p.ends_with("watched.txt")))),
            "delete event not journaled; journal = {:?}",
            journal_lines(&config)
        );

        // The multi-event burst from one logical write must have
        // journaled exactly one content line (dedup fix, end to end).
        let content_lines: Vec<_> = journal_lines(&config)
            .into_iter()
            .filter(|l| {
                l["sha"].is_string()
                    && l["path"].as_str().is_some_and(|p| p.ends_with("watched.txt"))
            })
            .collect();
        assert_eq!(
            content_lines.len(),
            1,
            "one logical write must journal one content line: {content_lines:?}"
        );

        // Blob restorable.
        let sha = content_lines[0]["sha"].as_str().unwrap().to_string();
        let blob = std::fs::read_to_string(config.objects_dir().join(&sha)).unwrap();
        assert_eq!(blob, "version one");

        drop(handle);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn watcher_ignores_excluded_dirs() {
        let root = unique_dir("excl_root");
        let base = unique_dir("excl_base");
        std::fs::create_dir_all(root.join("target")).unwrap();
        let config = test_config(root.clone(), base.clone());
        let handle = spawn_watcher(config.clone()).unwrap();

        std::fs::write(root.join("target/build.log"), "noise").unwrap();
        std::fs::write(root.join("kept.txt"), "signal").unwrap();

        // Wait until the non-excluded file shows up, then assert the
        // excluded one never did.
        let mut seen_kept = false;
        for _ in 0..100 {
            let lines = journal_lines(&config);
            if lines
                .iter()
                .any(|l| l["path"].as_str().is_some_and(|p| p.ends_with("kept.txt")))
            {
                seen_kept = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(seen_kept, "kept.txt must be journaled");
        let lines = journal_lines(&config);
        assert!(
            !lines
                .iter()
                .any(|l| l["path"].as_str().is_some_and(|p| p.contains("build.log"))),
            "excluded target/ file must not be journaled; journal = {lines:?}"
        );

        drop(handle);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn lua_surface_history_restore_and_handle() {
        let root = unique_dir("lua_root");
        let base = unique_dir("lua_base");
        let config = test_config(root.clone(), base.clone());
        let mut state = DedupState::default();
        let file = root.join("note.txt");
        std::fs::write(&file, "hello lua").unwrap();
        record_snapshot(&config, &mut state, &file, "create").unwrap();

        let lua = Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let code = format!(
            r#"
            local opts = {{ store_dir = "{base}" }}
            local versions = std.watch.history("note.txt", opts)
            assert(#versions == 1, "expected 1 version, got " .. #versions)
            local v = versions[1]
            local r = std.watch.restore("{dest}", v.sha, opts)
            assert(r.size == 9, "size " .. tostring(r.size))
            local h = std.watch.start("{root}", opts)
            assert(h:running())
            h:stop()
            assert(not h:running())
            return "ok"
            "#,
            base = base.display(),
            dest = root.join("restored.txt").display(),
            root = root.display(),
        );
        let out: String = lua.load(&code).eval().unwrap();
        assert_eq!(out, "ok");
        assert_eq!(
            std::fs::read_to_string(root.join("restored.txt")).unwrap(),
            "hello lua"
        );

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&base);
    }
}
