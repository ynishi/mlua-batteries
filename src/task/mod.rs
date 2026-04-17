//! `std.task` — structured async task primitives for Lua scripts.
//!
//! # API surface
//!
//! - `std.task.spawn(fn, opts?)` — fire-and-forget child, returns `Handle`
//! - `std.task.sleep(ms)` / `std.task.yield()` — cancel-aware suspension
//! - `std.task.checkpoint()` — bare cancel yield point
//! - `std.task.scope(name?, fn)` — structured nursery
//! - `std.task.with_timeout(ms, fn, opts?)` — scope with deadline
//! - `std.task.cancel_token()` — standalone `CancelToken`
//! - `std.task.current()` — `{id, name, cancelled}`
//! - `Scope:spawn`, `:cancel`, `:token`, `.name`
//! - `Handle:join`, `:abort`, `:is_finished`, `:elapsed`, `.id`, `.name`
//! - `CancelToken:cancel`, `:is_cancelled`, `:check`
//!
//! # Structured concurrency
//!
//! `task.scope(fn)` creates a `Scope`, installs it as the task-local
//! **current scope** (`LOCAL_SCOPE`) for the duration of `fn(scope)`, and
//! — regardless of how `fn` exits — waits for every task spawned into that
//! scope to finish before returning.  On error the scope's `CancelToken`
//! is set so cooperative children unwind; `scope` itself performs **no
//! hard abort** (matches Trio / Swift `TaskGroup` / Kotlin
//! `coroutineScope` / tokio-util `TaskTracker`).  A non-cooperative child
//! (never reaching a cancel checkpoint) therefore blocks the scope
//! indefinitely — the caller is expected to wrap with `task.with_timeout`
//! to bound teardown.  Top-level `task.spawn` attaches to the VM root
//! scope when no scope is installed.
//!
//! `LOCAL_SCOPE` is propagated via `tokio::task_local!` rather than a
//! shared VM-wide stack so a grandchild spawned with `task.spawn`
//! attaches to the correct ancestor scope even when concurrent siblings
//! are running their own `task.scope` bodies across `await` points.
//!
//! # Cancellation
//!
//! Cancellation is **cooperative + level-triggered** (Trio model): every
//! `std.task.*` suspension point (`sleep`, `yield`, `checkpoint`, and the
//! `coroutine` driver's `coroutine.yield`) consults the effective cancel
//! token (see [`effective_token`]) and raises `"task cancelled"`
//! when it fires.  `pcall`-swallowed cancellations reappear at the next
//! checkpoint, so cleanup code cannot accidentally suppress a cancel.
//!
//! `task.with_timeout(ms, fn, opts?)` layers a **3-stage graceful-abort**
//! pattern on top (Kubernetes / ASP.NET Core / Spring Boot):
//!   1. deadline trips → `token.cancel()`
//!   2. `drain_scope` runs under `timeout(grace_ms)` (default from
//!      [`TaskConfig::grace_ms`], 1 s if the host did not override)
//!   3. any child still alive is hard-aborted via tokio `AbortHandle`
//!      and a final drain reaps it
//!
//! `grace_ms = 0` yields strict/immediate-abort semantics.  The scope's
//! RAII `ScopeGuard` also aborts children if the entire scope future
//! is dropped mid-await (outer timeout, VM teardown).
//!
//! # Configuration
//!
//! [`TaskConfig`] carries the runtime-tunable knobs (default driver,
//! default grace window).  This crate **does not read environment
//! variables** — the host (e.g. `agent-block`) is expected to build a
//! `TaskConfig` from its own env / config sources and pass it to
//! [`register_with`].  [`register`] is the convenience entry point that
//! uses defaults.
//!
//! # Runtime contract
//!
//! Must run inside a `tokio::task::LocalSet` driven by a current-thread
//! runtime.  All primitives are `!Send`; tasks share an
//! `Rc<RefCell<Scope>>` across task-locals.
//!
//! # Drivers
//!
//! - `async_fn` (default) — drives the user function via
//!   `Function::call_async`, so `sleep` / `yield` / `checkpoint` suspend
//!   through mlua's async bridge.
//! - `coroutine` (opt-in) — drives a raw Lua thread via `Thread::resume`
//!   in a loop; `coroutine.yield()` yields cooperatively and
//!   `coroutine.yield(ms)` sleeps (cancel-aware).  Selected via
//!   `opts.driver = "coroutine"` per-spawn or [`TaskConfig::default_driver`].
//!
//! Every task is wrapped in a `tracing::info_span!("task", id, name,
//! driver)` so downstream tool logs (sh / mesh / mcp / sql) carry task
//! context.  `std.task.current()` inside a spawned task returns
//! `{id, name, cancelled}` for Lua-side introspection.
//!
//! # Module layout
//!
//! - `cancel` — `CancelToken` + `effective_token` + cancel-aware sleep/yield
//! - `scope`  — `Scope` + `ScopeGuard` + `drain_scope` / `abort_all` + `ScopeHandle`
//! - `driver` — `Driver` enum + `parse_opts` + `run_coroutine` + `Handle` + `spawn_into`
//! - `api`    — Lua-facing `std.task.*` callables (`spawn`, `scope`, `with_timeout`, …)

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use mlua::prelude::*;

mod api;
mod cancel;
mod driver;
mod scope;

use scope::Scope;

pub use cancel::{effective_token, CancelToken};
pub use driver::Driver;

/// Runtime configuration for the task bridge.
///
/// The host is responsible for constructing this (typically from its own
/// environment / config sources) and passing it to [`register_with`].
/// Defaults match the historical behaviour of the agent-block host
/// before extraction (driver = `AsyncFn`, grace = 1000 ms).
#[derive(Clone, Debug)]
pub struct TaskConfig {
    /// Driver used by `std.task.spawn` when the caller does not pass
    /// `opts.driver`.  Per-call `opts.driver` always wins.
    pub default_driver: Driver,
    /// Grace window (cooperative cancel → hard abort) used by
    /// `std.task.with_timeout` when the caller does not pass
    /// `opts.grace_ms`.  Set to `0` for strict / immediate-abort semantics.
    pub grace_ms: u64,
}

impl Default for TaskConfig {
    fn default() -> Self {
        Self {
            default_driver: Driver::AsyncFn,
            grace_ms: 1000,
        }
    }
}

/// Lua-visible descriptor returned by `std.task.current()`.  Carried via
/// the `TASK_INFO` task-local rather than threaded through the Lua function
/// signature so any frame inside a spawned task can query it.
#[derive(Clone)]
pub(crate) struct TaskInfo {
    pub(crate) id: String,
    pub(crate) name: Option<String>,
}

tokio::task_local! {
    /// Set by `spawn_into` for the duration of a spawned task so that
    /// `task.checkpoint()` can consult the task's cancellation token
    /// without the caller threading it through manually.
    pub(crate) static TASK_TOKEN: CancelToken;
    /// Set by `spawn_into` for the duration of a spawned task so that
    /// `std.task.current()` can return id/name without threading them
    /// through the Lua function signature.
    pub(crate) static TASK_INFO: TaskInfo;
    /// The scope enclosing the currently-running task body.  Set by
    /// `task.scope` / `task.with_timeout` for their user function, and
    /// by `spawn_into` for each spawned child — so `task.spawn` always
    /// attaches to the correct scope without a shared VM-wide stack
    /// (which would interleave across concurrent tasks).
    pub(crate) static LOCAL_SCOPE: Rc<RefCell<Scope>>;
}

fn duration_to_ms(d: Duration) -> f64 {
    // f64 loses precision past ~2^53 ns (~104 days).  Acceptable because
    // `Handle::elapsed()` is short-lived observation of a live task, not a
    // persisted timestamp; any task whose elapsed time approaches that
    // range has already broken the single-thread LocalSet contract by
    // starving sibling tasks.
    (d.as_nanos() as f64) / 1_000_000.0
}

/// Convert a Lua `ms` argument into a `Duration`, rejecting non-finite
/// (NaN / ±∞), negative, and out-of-range values.  `ctx` is the caller
/// name used in the error message.
fn ms_to_duration(ms: f64, ctx: &str) -> LuaResult<Duration> {
    if !ms.is_finite() || ms < 0.0 {
        return Err(LuaError::external(format!(
            "{ctx}: invalid duration (ms={ms})"
        )));
    }
    const MAX_MS: f64 = u64::MAX as f64 / 1_000_000.0;
    if ms > MAX_MS {
        return Err(LuaError::external(format!(
            "{ctx}: duration out of range (ms={ms}, max≈{MAX_MS:.3e})"
        )));
    }
    Ok(Duration::from_nanos((ms * 1_000_000.0) as u64))
}

fn lua_to_string(v: &LuaValue, ctx: &str) -> LuaResult<String> {
    match v {
        LuaValue::String(s) => Ok(s.to_str()?.to_string()),
        other => Err(LuaError::external(format!(
            "{ctx}: expected string, got {}",
            other.type_name()
        ))),
    }
}

/// Register `std.task` into the given Lua VM with default configuration.
///
/// Equivalent to [`register_with`] with [`TaskConfig::default()`].  The
/// caller is expected to have already created a `std` table on the
/// globals (this matches how the host wires sibling bridges).
pub fn register(lua: &Lua) -> LuaResult<()> {
    register_with(lua, TaskConfig::default())
}

/// Register `std.task` with caller-provided [`TaskConfig`].
///
/// Stores the config in `lua.app_data` so the Lua-facing primitives can
/// consult it without threading it through every closure capture.
pub fn register_with(lua: &Lua, cfg: TaskConfig) -> LuaResult<()> {
    lua.set_app_data::<TaskConfig>(cfg);

    // Install the root scope as app_data.  The root scope lives for the VM
    // lifetime and catches top-level `task.spawn` calls that are not inside
    // any `task.scope` body.  Its Drop triggers a last-resort abort on
    // outstanding fire-and-forget tasks during VM teardown.
    let root = Scope::new(Some("root".to_string()));
    lua.set_app_data::<Rc<RefCell<Scope>>>(root);

    let task = lua.create_table()?;
    task.set("spawn", lua.create_function(api::spawn)?)?;
    task.set("sleep", lua.create_async_function(api::sleep)?)?;
    task.set("yield", lua.create_async_function(api::yield_now)?)?;
    task.set("checkpoint", lua.create_async_function(api::checkpoint)?)?;
    task.set("cancel_token", lua.create_function(api::cancel_token)?)?;
    task.set("current", lua.create_function(api::current)?)?;
    task.set("scope", lua.create_async_function(api::task_scope)?)?;
    task.set(
        "with_timeout",
        lua.create_async_function(api::with_timeout)?,
    )?;

    let std_ns: LuaTable = lua.globals().get("std")?;
    std_ns.set("task", task)?;
    Ok(())
}

/// Read the registered [`TaskConfig`] (set by [`register_with`]).  Falls
/// back to defaults if the bridge was not registered, which keeps the
/// internal helpers infallible at the cost of silently using defaults
/// when called from a misconfigured VM.
pub(crate) fn task_config(lua: &Lua) -> TaskConfig {
    lua.app_data_ref::<TaskConfig>()
        .map(|c| c.clone())
        .unwrap_or_default()
}
