# Changelog

All notable changes to `mlua-batteries` will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.0] - 2026-09-04

Current track: `rusqlite 0.37` / `libsqlite3-sys 0.35`.

### Changed

- **BREAKING** — `std.sql` / `std.kv` are now built on
  [`rusqlite-isle`](https://crates.io/crates/rusqlite-isle) instead of a
  hand-rolled `Arc<Mutex<Connection>>` + `spawn_blocking` + `InterruptHandle`
  arrangement. The Lua-side API (`std.sql.query` / `exec` / `null`,
  `std.kv.get` / `set` / `delete` / `list`) is unchanged; what changes is how
  a host wires the bridges:
  - `sql::register(lua, conn, interrupt)` → `sql::register(lua, isle)`, where
    `isle` is an `AsyncIsle` clone. Same for `register_with`.
  - `kv::register` / `kv::register_with` are now **`async`** — the `__kv`
    table is created through the isle before `std.kv` is exposed. Hosts that
    prefer to do it at open time can call the new public
    `kv::init_schema` from the isle's `init` closure.
  - The host now owns an `AsyncIsle` + `AsyncIsleDriver` (the driver shuts the
    connection thread down) instead of a `Connection` and its
    `InterruptHandle`. File path, `busy_timeout` and `journal_mode` remain
    host-side concerns, applied in the isle's `init` closure or through
    `AsyncIsleBuilder::wal`.
  - Cancellation semantics are preserved: jobs are submitted with
    `spawn_call`, whose task cancels on drop, so an enclosing `task.scope` /
    `task.with_timeout` firing — or the `SqlConfig` query timeout elapsing —
    still interrupts the running statement. `SqlConfig` itself is unchanged.
  - `std.kv` writes (`set` / `delete`) now run inside a `BEGIN IMMEDIATE`
    transaction, taking the write lock up front instead of risking the
    timeout-bypassing deferred upgrade under cross-process contention.
- **BREAKING** — the `sql` feature no longer pulls a bare `rusqlite`
  dependency of its own choosing; it pulls the `rusqlite-isle` minor and the
  matching `rusqlite` cluster for this release line's track (see below).

### Added

- `sqlite_backend` module: re-exports the active `rusqlite` and
  `rusqlite_isle`, so hosts can name the bridge's exact versions
  (`mlua_batteries::sqlite_backend::rusqlite_isle::AsyncIsle`) without
  declaring a second dependency that could drift onto another cluster.
- `sqlite-bundled` feature, **enabled by default**, forwarding to
  `rusqlite/bundled`. Opt out with `default-features = false` to link the
  system SQLite. It is a weak (`dep?/`) forward, so it is inert unless `sql`
  is enabled.

### Notes

- **SQLite version tracks.** `libsqlite3-sys` declares `links = "sqlite3"`,
  and cargo rejects a manifest that names two rusqlite clusters as optional
  dependencies even when only one is activated — the check happens during
  dependency resolution, not after feature activation. Serving several
  clusters therefore requires separate published versions, as `rusqlite-isle`
  does; it cannot be done with mutually exclusive features inside one
  version. This line (`0.4`) tracks `rusqlite 0.37`.

## [0.3.1] - 2026-09-03

### Added
- `std.proc`: typed pipeline spawn (feature `proc`).
- `std.watch`: versioning file watcher (feature `watch`).
- `std.fs.rename(src, dst)`: atomic rename via POSIX `rename(2)` (feature `fs`).
- `std.fs.symlink(target, linkpath)`: create a POSIX symlink via `symlink(2)`; dangling targets are allowed (feature `fs`, Unix only).
- `json.encode` honours the `dkjson`-style `__jsontype` metatable tag on
  **empty** tables: `setmetatable({}, {__jsontype = "array"})` encodes as
  `[]`, and `__jsontype = "object"` states the default (`{}`) explicitly.
  An empty table carrying the metatable mlua's serde bridge attaches to
  sequences (single entry `__metatable = false`) also encodes as `[]`.
  Tables with contents are classified by their contents as before — the tag
  cannot turn a map into an array or vice versa.
- `json.array()`: returns a fresh empty table tagged as a JSON array, so
  scripts can produce `[]` without writing the `setmetatable` boilerplate.
- The same tag is honoured by the NULL-preserving converters behind
  `std.sql` / `std.kv`, sharing the metatable instance with `std.json`, so a
  value stored with `kv.set` and read back with `kv.get` keeps its empty
  arrays instead of turning them into `{}` (features `sql` / `kv`).  SQL NULL
  handling (`std.sql.null`) is unchanged.
- The list-shaped returns of those bridges are tagged when empty for the same
  reason: a zero-row `sql.query` result and an empty `kv.list` result now
  encode as `[]`.  Non-empty results are returned untagged, as before.

### Changed
- `json.decode("[]")` now returns an empty table carrying the shared
  `{__jsontype = "array"}` metatable (previously a bare table), closing the
  round-trip: `json.encode(json.decode("[]")) == "[]"` where it used to
  produce `"{}"`.  This applies to empty arrays at any nesting depth.
  The metatable is unprotected and carries no `__index` / `__newindex`, so
  decoded values keep behaving like plain tables; code that asserts
  `getmetatable(decoded) == nil` for an empty array is affected.
  Non-empty arrays are returned untagged, as before.
- `kv.get` likewise returns tagged tables for empty arrays stored in the
  value, and `sql.query` / `kv.list` return a tagged table when the result is
  empty; the same `getmetatable(...) == nil` caveat applies there.

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
