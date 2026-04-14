# Changelog

All notable changes to `mlua-batteries` will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/ynishi/mlua-batteries/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/ynishi/mlua-batteries/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/ynishi/mlua-batteries/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/ynishi/mlua-batteries/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/ynishi/mlua-batteries/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/ynishi/mlua-batteries/releases/tag/v0.1.0
