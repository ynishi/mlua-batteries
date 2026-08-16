//! Typed argv pipeline spawn — `std.proc`.
//!
//! Executes a pipeline of processes **without going through a shell**:
//! each stage is spawned directly via `std::process::Command` from a
//! pre-split argv array, and adjacent stages are connected with raw
//! stdio pipes. Because no shell is involved, `$()` / backticks / `;` /
//! `&&` / heredocs / implicit redirects simply do not exist as syntax.
//!
//! Ported from mlua-mcp-server's guard-mcp v0 `proc` binding; the Lua
//! surface is unchanged apart from living under the `std` namespace.
//!
//! ```lua
//! local r = std.proc.pipeline(
//!     {
//!         { argv = { "cargo", "test", "-p", "foo" }, env = { RUST_LOG = "off" } },
//!         { argv = { "head", "-50" } },
//!     },
//!     {
//!         cwd = "/abs/or/relative",
//!         stdin_from  = { path = "input.txt" },
//!         stdout_to   = { path = "out.log", append = false },
//!         timeout_secs = 120,
//!     }
//! )
//! -- r = {
//! --   ok = true|false,          -- all stages exited 0 and no timeout
//! --   timed_out = true|false,
//! --   duration_ms = <total>,
//! --   stages = {
//! --     { exit_code = 0|nil, stdout = "...", stderr = "...", duration_ms = n },
//! --     ...
//! --   },
//! -- }
//! ```
//!
//! Semantics:
//! - word splitting is the **caller's** responsibility: argv is only
//!   accepted as a pre-split array; no single-string form is provided
//! - only the **last** stage's stdout is captured (intermediate stdout
//!   feeds the next stage's stdin); every stage's stderr is captured
//! - captured streams are tail-capped at [`OUTPUT_TAIL_MAX_BYTES`]
//! - `stdout_to` is the only typed redirect; when present the last
//!   stage's stdout goes to the file and its captured `stdout` is empty
//! - glob expansion is **not implemented** (callers pass literal paths)
//! - foreground only; the whole pipeline is bounded by `timeout_secs`
//!   (default [`DEFAULT_TIMEOUT_SECS`]); on timeout all remaining
//!   children are killed and `timed_out = true`
//!
//! **Policy note**: process spawning is not routed through any
//! [`crate::policy`] check — gate invocations at a higher layer (the
//! guard-mcp host evaluates a deny-rule policy before calling this).

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use mlua::prelude::*;

/// Default pipeline timeout when `opts.timeout_secs` is omitted.
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Tail cap for each captured stream (last-stage stdout / per-stage stderr).
pub const OUTPUT_TAIL_MAX_BYTES: usize = 64 * 1024;

/// Poll interval for the wait loop.
const POLL_INTERVAL: Duration = Duration::from_millis(15);

// ─── Spec types ──────────────────────────────────────────────────────────

/// One pipeline stage: pre-split argv + env overrides (appended to the
/// inherited environment, never clearing it).
#[derive(Debug, Clone)]
pub struct StageSpec {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Typed redirect target for the last stage's stdout.
#[derive(Debug, Clone)]
pub struct StdoutTo {
    pub path: PathBuf,
    pub append: bool,
}

/// Full pipeline invocation.
#[derive(Debug, Clone)]
pub struct PipelineSpec {
    pub stages: Vec<StageSpec>,
    pub cwd: Option<PathBuf>,
    pub stdin_from: Option<PathBuf>,
    pub stdout_to: Option<StdoutTo>,
    pub timeout: Duration,
}

// ─── Result types ────────────────────────────────────────────────────────

/// Per-stage outcome. `exit_code` is `None` when the process was killed
/// (timeout) or its status could not be read.
#[derive(Debug, Clone, PartialEq)]
pub struct StageResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

/// Whole-pipeline outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineResult {
    pub ok: bool,
    pub timed_out: bool,
    pub duration_ms: u64,
    pub stages: Vec<StageResult>,
}

// ─── Core runner (pure Rust, unit-testable without Lua) ──────────────────

/// Spawn a reader thread that drains `r` into a rolling tail buffer
/// capped at [`OUTPUT_TAIL_MAX_BYTES`].
fn spawn_tail_reader<R: Read + Send + 'static>(mut r: R) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match r.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() > OUTPUT_TAIL_MAX_BYTES {
                        let excess = buf.len() - OUTPUT_TAIL_MAX_BYTES;
                        buf.drain(..excess);
                    }
                }
                Err(_) => break,
            }
        }
        buf
    })
}

/// Execute the pipeline described by `spec`.
///
/// Returns `Err(String)` only for **setup** failures (empty stages,
/// unreadable `stdin_from`, unwritable `stdout_to`, spawn failure).
/// Non-zero exits and timeouts are reported inside the `Ok` result
/// (`ok = false` / `timed_out = true`), never as `Err`.
pub fn run_pipeline(spec: &PipelineSpec) -> Result<PipelineResult, String> {
    if spec.stages.is_empty() {
        return Err("pipeline must contain at least one stage".to_string());
    }
    for (i, st) in spec.stages.iter().enumerate() {
        if st.argv.is_empty() {
            return Err(format!("stage {}: argv must be non-empty", i + 1));
        }
    }

    // Relative aux-file paths resolve against `cwd` when given (matching
    // the working directory the stages themselves run in).
    let resolve = |p: &Path| -> PathBuf {
        if p.is_absolute() {
            p.to_path_buf()
        } else if let Some(cwd) = &spec.cwd {
            cwd.join(p)
        } else {
            p.to_path_buf()
        }
    };

    let start = Instant::now();
    let n = spec.stages.len();
    let mut children: Vec<Child> = Vec::with_capacity(n);
    let mut stderr_readers: Vec<Option<std::thread::JoinHandle<Vec<u8>>>> = Vec::with_capacity(n);
    let mut prev_stdout: Option<std::process::ChildStdout> = None;
    let mut last_stdout_reader: Option<std::thread::JoinHandle<Vec<u8>>> = None;

    for (i, st) in spec.stages.iter().enumerate() {
        let is_last = i == n - 1;
        let mut cmd = Command::new(&st.argv[0]);
        cmd.args(&st.argv[1..]);
        for (k, v) in &st.env {
            cmd.env(k, v);
        }
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }

        // stdin: first stage from typed `stdin_from` (or null); later
        // stages from the previous stage's stdout pipe.
        if i == 0 {
            match &spec.stdin_from {
                Some(p) => {
                    let path = resolve(p);
                    let f = File::open(&path)
                        .map_err(|e| format!("stdin_from {}: {e}", path.display()))?;
                    cmd.stdin(Stdio::from(f));
                }
                None => {
                    cmd.stdin(Stdio::null());
                }
            }
        } else {
            let prev = prev_stdout
                .take()
                .ok_or_else(|| "internal error: missing inter-stage pipe".to_string())?;
            cmd.stdin(Stdio::from(prev));
        }

        // stdout: intermediate stages pipe into the next stage; the last
        // stage either writes to the typed redirect file or is captured.
        if is_last {
            match &spec.stdout_to {
                Some(t) => {
                    let path = resolve(&t.path);
                    let f = if t.append {
                        OpenOptions::new().create(true).append(true).open(&path)
                    } else {
                        File::create(&path)
                    }
                    .map_err(|e| format!("stdout_to {}: {e}", path.display()))?;
                    cmd.stdout(Stdio::from(f));
                }
                None => {
                    cmd.stdout(Stdio::piped());
                }
            }
        } else {
            cmd.stdout(Stdio::piped());
        }
        cmd.stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                // Kill anything already running before bailing.
                for c in children.iter_mut() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                return Err(format!("stage {} ({}): spawn failed: {e}", i + 1, st.argv[0]));
            }
        };

        if !is_last {
            prev_stdout = child.stdout.take();
        } else if spec.stdout_to.is_none() {
            last_stdout_reader = child.stdout.take().map(spawn_tail_reader);
        }
        stderr_readers.push(child.stderr.take().map(spawn_tail_reader));
        children.push(child);
    }

    // Wait loop with deadline. Per-stage duration is measured from
    // pipeline start to that stage's observed exit.
    let deadline = start + spec.timeout;
    let mut statuses: Vec<Option<(Option<i32>, u64)>> = vec![None; n];
    let mut timed_out = false;
    loop {
        let mut all_done = true;
        for (i, child) in children.iter_mut().enumerate() {
            if statuses[i].is_some() {
                continue;
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    statuses[i] = Some((status.code(), start.elapsed().as_millis() as u64));
                }
                Ok(None) => all_done = false,
                Err(_) => {
                    statuses[i] = Some((None, start.elapsed().as_millis() as u64));
                }
            }
        }
        if all_done {
            break;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            for (i, child) in children.iter_mut().enumerate() {
                if statuses[i].is_none() {
                    let _ = child.kill();
                    let _ = child.wait();
                    statuses[i] = Some((None, start.elapsed().as_millis() as u64));
                }
            }
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    // All children are dead here, so the pipes are closed and the reader
    // threads terminate; joining cannot deadlock.
    let last_stdout_bytes = last_stdout_reader
        .map(|h| h.join().unwrap_or_default())
        .unwrap_or_default();

    let mut stages_out = Vec::with_capacity(n);
    for (i, handle) in stderr_readers.into_iter().enumerate() {
        let stderr_bytes = handle.map(|h| h.join().unwrap_or_default()).unwrap_or_default();
        let (exit_code, duration_ms) =
            statuses[i].unwrap_or((None, start.elapsed().as_millis() as u64));
        let stdout = if i == n - 1 {
            String::from_utf8_lossy(&last_stdout_bytes).into_owned()
        } else {
            String::new()
        };
        stages_out.push(StageResult {
            exit_code,
            stdout,
            stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
            duration_ms,
        });
    }

    let ok = !timed_out && stages_out.iter().all(|s| s.exit_code == Some(0));
    Ok(PipelineResult {
        ok,
        timed_out,
        duration_ms: start.elapsed().as_millis() as u64,
        stages: stages_out,
    })
}

// ─── Lua binding ─────────────────────────────────────────────────────────

/// Parse the Lua-side `(stages, opts)` arguments into a [`PipelineSpec`].
fn parse_pipeline_spec(stages: LuaTable, opts: Option<LuaTable>) -> Result<PipelineSpec, String> {
    let mut out_stages = Vec::new();
    for (idx, stage) in stages.sequence_values::<LuaTable>().enumerate() {
        let i = idx + 1;
        let stage = stage.map_err(|e| format!("stage {i}: not a table: {e}"))?;
        let argv_tbl: LuaTable = stage
            .get("argv")
            .map_err(|e| format!("stage {i}: argv missing or not a table: {e}"))?;
        let mut argv = Vec::new();
        for v in argv_tbl.sequence_values::<String>() {
            argv.push(v.map_err(|e| format!("stage {i}: argv item not a string: {e}"))?);
        }
        if argv.is_empty() {
            return Err(format!("stage {i}: argv must be non-empty"));
        }
        let mut env = Vec::new();
        if let Ok(env_tbl) = stage.get::<LuaTable>("env") {
            for pair in env_tbl.pairs::<String, String>() {
                let (k, v) = pair.map_err(|e| format!("stage {i}: env entry: {e}"))?;
                env.push((k, v));
            }
        }
        out_stages.push(StageSpec { argv, env });
    }

    let mut cwd = None;
    let mut stdin_from = None;
    let mut stdout_to = None;
    let mut timeout = Duration::from_secs(DEFAULT_TIMEOUT_SECS);
    if let Some(o) = opts {
        if let Ok(s) = o.get::<String>("cwd") {
            cwd = Some(PathBuf::from(s));
        }
        if let Ok(t) = o.get::<LuaTable>("stdin_from") {
            let p: String = t
                .get("path")
                .map_err(|e| format!("stdin_from.path: {e}"))?;
            stdin_from = Some(PathBuf::from(p));
        }
        if let Ok(t) = o.get::<LuaTable>("stdout_to") {
            let p: String = t
                .get("path")
                .map_err(|e| format!("stdout_to.path: {e}"))?;
            let append = t.get::<bool>("append").unwrap_or(false);
            stdout_to = Some(StdoutTo {
                path: PathBuf::from(p),
                append,
            });
        }
        if let Ok(secs) = o.get::<f64>("timeout_secs") {
            if secs > 0.0 {
                timeout = Duration::from_secs_f64(secs);
            }
        }
    }

    Ok(PipelineSpec {
        stages: out_stages,
        cwd,
        stdin_from,
        stdout_to,
        timeout,
    })
}

/// Convert a [`PipelineResult`] into the documented Lua result table.
fn pipeline_result_to_lua(lua: &Lua, r: &PipelineResult) -> LuaResult<LuaTable> {
    let out = lua.create_table()?;
    out.set("ok", r.ok)?;
    out.set("timed_out", r.timed_out)?;
    out.set("duration_ms", r.duration_ms)?;
    let stages = lua.create_table()?;
    for (i, s) in r.stages.iter().enumerate() {
        let st = lua.create_table()?;
        match s.exit_code {
            Some(code) => st.set("exit_code", code)?,
            None => st.set("exit_code", LuaValue::Nil)?,
        }
        st.set("stdout", s.stdout.as_str())?;
        st.set("stderr", s.stderr.as_str())?;
        st.set("duration_ms", s.duration_ms)?;
        stages.set(i + 1, st)?;
    }
    out.set("stages", stages)?;
    Ok(out)
}

/// Module entry point: builds the `proc` table (`proc.pipeline`).
pub fn module(lua: &Lua) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;
    t.set(
        "pipeline",
        lua.create_function(|lua, (stages, opts): (LuaTable, Option<LuaTable>)| {
            let spec = parse_pipeline_spec(stages, opts).map_err(LuaError::external)?;
            let result = run_pipeline(&spec).map_err(LuaError::external)?;
            pipeline_result_to_lua(lua, &result)
        })?,
    )?;
    Ok(t)
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(argv: &[&str]) -> StageSpec {
        StageSpec {
            argv: argv.iter().map(|s| s.to_string()).collect(),
            env: Vec::new(),
        }
    }

    fn spec(stages: Vec<StageSpec>) -> PipelineSpec {
        PipelineSpec {
            stages,
            cwd: None,
            stdin_from: None,
            stdout_to: None,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        }
    }

    fn unique_tmp(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("mlua_bat_proc_{}_{}_{name}", std::process::id(), nanos))
    }

    #[test]
    fn single_stage_captures_stdout() {
        let r = run_pipeline(&spec(vec![stage(&["echo", "hello"])])).unwrap();
        assert!(r.ok);
        assert!(!r.timed_out);
        assert_eq!(r.stages.len(), 1);
        assert_eq!(r.stages[0].exit_code, Some(0));
        assert_eq!(r.stages[0].stdout, "hello\n");
    }

    #[test]
    fn two_stage_pipe_connects_stdio() {
        let r = run_pipeline(&spec(vec![
            stage(&["echo", "hello world"]),
            stage(&["tr", "a-z", "A-Z"]),
        ]))
        .unwrap();
        assert!(r.ok);
        assert_eq!(r.stages.len(), 2);
        // Intermediate stdout goes to the pipe, not the capture buffer.
        assert_eq!(r.stages[0].stdout, "");
        assert_eq!(r.stages[1].stdout, "HELLO WORLD\n");
        assert_eq!(r.stages[0].exit_code, Some(0));
        assert_eq!(r.stages[1].exit_code, Some(0));
    }

    #[test]
    fn nonzero_exit_reported_not_err() {
        let r = run_pipeline(&spec(vec![stage(&["false"])])).unwrap();
        assert!(!r.ok);
        assert!(!r.timed_out);
        assert_eq!(r.stages[0].exit_code, Some(1));
    }

    #[test]
    fn timeout_kills_pipeline() {
        let mut s = spec(vec![stage(&["sleep", "5"])]);
        s.timeout = Duration::from_millis(300);
        let start = Instant::now();
        let r = run_pipeline(&s).unwrap();
        assert!(r.timed_out);
        assert!(!r.ok);
        assert_eq!(r.stages[0].exit_code, None);
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "kill must not wait for sleep to finish"
        );
    }

    #[test]
    fn stdout_to_writes_file() {
        let out_path = unique_tmp("stdout_to.log");
        let mut s = spec(vec![stage(&["echo", "to-file"])]);
        s.stdout_to = Some(StdoutTo {
            path: out_path.clone(),
            append: false,
        });
        let r = run_pipeline(&s).unwrap();
        assert!(r.ok);
        // Redirected stdout is not captured.
        assert_eq!(r.stages[0].stdout, "");
        let content = std::fs::read_to_string(&out_path).unwrap();
        assert_eq!(content, "to-file\n");
        let _ = std::fs::remove_file(&out_path);
    }

    #[test]
    fn stdout_to_append_appends() {
        let out_path = unique_tmp("append.log");
        std::fs::write(&out_path, "first\n").unwrap();
        let mut s = spec(vec![stage(&["echo", "second"])]);
        s.stdout_to = Some(StdoutTo {
            path: out_path.clone(),
            append: true,
        });
        let r = run_pipeline(&s).unwrap();
        assert!(r.ok);
        let content = std::fs::read_to_string(&out_path).unwrap();
        assert_eq!(content, "first\nsecond\n");
        let _ = std::fs::remove_file(&out_path);
    }

    #[test]
    fn stage_env_is_visible_to_process() {
        let mut st = stage(&["printenv", "MLUA_BAT_PROC_TEST_VAR"]);
        st.env
            .push(("MLUA_BAT_PROC_TEST_VAR".to_string(), "guarded".to_string()));
        let r = run_pipeline(&spec(vec![st])).unwrap();
        assert!(r.ok);
        assert_eq!(r.stages[0].stdout, "guarded\n");
    }

    #[test]
    fn stdin_from_feeds_first_stage() {
        let in_path = unique_tmp("stdin.txt");
        std::fs::write(&in_path, "typed input").unwrap();
        let mut s = spec(vec![stage(&["cat"])]);
        s.stdin_from = Some(in_path.clone());
        let r = run_pipeline(&s).unwrap();
        assert!(r.ok);
        assert_eq!(r.stages[0].stdout, "typed input");
        let _ = std::fs::remove_file(&in_path);
    }

    #[test]
    fn spawn_failure_is_setup_err() {
        let err = run_pipeline(&spec(vec![stage(&["/nonexistent/mlua_bat_no_such_bin"])]))
            .unwrap_err();
        assert!(err.contains("spawn failed"), "got: {err}");
    }

    #[test]
    fn empty_pipeline_is_setup_err() {
        let err = run_pipeline(&spec(vec![])).unwrap_err();
        assert!(err.contains("at least one stage"), "got: {err}");
    }

    #[test]
    fn captured_stdout_is_tail_capped() {
        // 200_000 bytes of zeros; tail cap keeps the last 64 KiB.
        let r = run_pipeline(&spec(vec![stage(&["head", "-c", "200000", "/dev/zero"])]))
            .unwrap();
        assert!(r.ok);
        assert_eq!(r.stages[0].stdout.len(), OUTPUT_TAIL_MAX_BYTES);
    }

    #[test]
    fn cwd_applies_to_stages() {
        let dir = unique_tmp("cwd_dir");
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = spec(vec![stage(&["pwd"])]);
        s.cwd = Some(dir.clone());
        let r = run_pipeline(&s).unwrap();
        assert!(r.ok);
        let printed = r.stages[0].stdout.trim();
        // Allow symlink-canonicalised tmp dirs (e.g. /tmp → /private/tmp).
        assert!(
            printed.ends_with(dir.file_name().unwrap().to_str().unwrap()),
            "pwd printed {printed:?}, expected suffix of {dir:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pipeline_reachable_from_lua_surface() {
        let ok: String = crate::util::test_eval(
            r#"
            local r = std.proc.pipeline({ { argv = { "echo", "hi" } } })
            return tostring(r.ok) .. ":" .. r.stages[1].stdout
            "#,
        );
        assert_eq!(ok, "true:hi\n");
    }
}
