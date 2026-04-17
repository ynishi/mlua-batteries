# Changelog

All notable changes to `mlua-batteries` will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0] - 2026-04-17

### Added
- `std.task`: Structured async task primitives for Lua scripts (feature `task`).
  Requires a `tokio` current-thread runtime driving a `LocalSet` (mlua-isle's
  `AsyncIsle` satisfies this).
  - `spawn`, `scope`, `with_timeout`, `sleep`, `yield`, `checkpoint`,
    `cancel_token`, `current`.
  - `Scope`, `Handle`, `CancelToken` userdata.
  - Cooperative + level-triggered cancellation (Trio model) at every
    `std.task.*` suspension point, including the `coroutine` driver.
  - 3-stage graceful abort in `with_timeout` (deadline → drain under
    `grace_ms` → hard-abort via tokio `AbortHandle`).
  - `TaskConfig` for host-tunable defaults (`default_driver`, `grace_ms`);
    no env-var reads inside the crate.
- `std.sql`: SQLite bridge built on `rusqlite` + `tokio::task::spawn_blocking`
  (feature `sql`, implies `task` + `json`).
  - `query(sql, params?) -> rows`, `exec(sql, params?) -> {affected, last_id}`,
    `std.sql.null` sentinel for SQL NULL.
  - Host owns the `rusqlite::Connection` and `InterruptHandle`; on cancel the
    crate calls `sqlite3_interrupt` so the blocking thread returns promptly.
  - Per-query timeout via `SqlConfig::query_timeout`; integrates with the
    enclosing `task.scope` / `task.with_timeout` cancel token.
- `std.kv`: SQLite-backed key-value store (feature `kv`, implies `sql`).
  - `get` / `set` / `delete` / `list` scoped by namespace.
  - Per-key updates (no whole-namespace rewrite); durability + atomicity via
    SQLite WAL; cross-process writes arbitrated by `busy_timeout`.
  - Shares `SqlConfig` (timeout + cancel) with `std.sql`; host supplies a
    dedicated `Connection` for KV scratch state.

## [0.2.2] - 2026-04-14

### Fixed
- `json.encode` now accepts `mlua::Value::NULL` (the `LightUserData(null_ptr)`
  sentinel produced by `mlua::serde::LuaSerdeExt::to_value`) and maps it to
  JSON `null`.  Previously any value produced via mlua's serde bridge that
  contained a JSON null — e.g. tool schemas fetched over MCP — would fail
  with `unsupported type for JSON conversion` on re-encode.
  Non-null `LightUserData` still errors (guardrail against silently
  serializing arbitrary pointers).
  ([#1](https://github.com/ynishi/mlua-batteries/issues/1))

## [0.2.1] - 2026-03-06

### Added
- `fs.read_binary` / `fs.write_binary` for raw byte I/O.
- `max_read_bytes` config guard on `fs.read` and `fs.read_binary`
  to bound memory usage on hostile or oversized inputs.

## [0.2.0] - 2026-03-05

### Added
- New modules: `string`, `regex`, `validate`, `log`, `uuid`, `base64`, `schema`.
  Covers common Lua scripting needs beyond the 0.1 core (fs/json/env/path/time).

## [0.1.2] - 2026-02-25

### Changed
- Metadata-only release (no functional changes).

## [0.1.1] - 2026-02-24

### Added
- README with module overview, sandboxing guide, and LLM usage notes.
- Contributing section.
- `readme` field in `Cargo.toml` for crates.io rendering.

## [0.1.0] - 2026-02-24

### Added
- Initial release.
- Modules:
  - `json`: encode/decode/read_file/write_file with depth limits.
  - `env`: safe overlay pattern (no `unsafe set_var`).
  - `path`: pure-computation path utilities.
  - `time`: `now` / `millis` / `sleep` / `measure` with configurable limits.
  - `fs`: full filesystem ops with glob/walk support.
  - `http`: `GET`/`POST`/`request` via `ureq` 3.
  - `llm`: multi-provider chat completion (OpenAI / Anthropic / Ollama).
  - `hash`: SHA-256 string and streaming file hashing.
  - `sandbox`: capability-based filesystem sandbox via `cap-std`.
- Policy system with trait-based access control:
  `PathPolicy`, `HttpPolicy`, `EnvPolicy`, `LlmPolicy`.

[Unreleased]: https://github.com/ynishi/mlua-batteries/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/ynishi/mlua-batteries/compare/v0.2.2...v0.3.0
[0.2.2]: https://github.com/ynishi/mlua-batteries/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/ynishi/mlua-batteries/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/ynishi/mlua-batteries/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/ynishi/mlua-batteries/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/ynishi/mlua-batteries/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/ynishi/mlua-batteries/releases/tag/v0.1.0
