# mlua-batteries

Batteries-included standard library modules for [mlua](https://github.com/mlua-rs/mlua).

Lua 5.4 scripts gain access to JSON, environment variables, filesystem, HTTP, hashing, and LLM chat completion — all behind a configurable policy layer that can sandbox untrusted code.

All APIs are **synchronous (blocking)**. No async runtime required.

## Modules

| Module | Feature flag | Description |
|--------|-------------|-------------|
| `json` | `json` | JSON encode / decode (`serde_json`) |
| `env` | `env` | Environment variable access (overlay-safe `set`) |
| `path` | `path` | Path manipulation (pure computation + `absolute`) |
| `time` | `time` | Timestamps, sleep, `measure` |
| `fs` | `fs` | File I/O, `walk`, `glob` (`walkdir` + `globset`) |
| `http` | `http` | HTTP client (`ureq`) |
| `hash` | `hash` | SHA-256 hashing (`sha2`) |
| `llm` | `llm` | Chat completion — OpenAI, Anthropic, Ollama |

Default features: `json`, `env`, `path`, `time`.
Enable everything: `full`.

## Quick start

```toml
[dependencies]
mlua-batteries = "0.1"
```

```rust
use mlua::prelude::*;

let lua = Lua::new();
mlua_batteries::register_all(&lua, "std").unwrap();

lua.load(r#"
    local data = std.json.decode('{"name":"Lua"}')
    print(data.name)                     -- Lua
    print(std.env.get("HOME"))           -- /Users/...
    print(std.path.join("/tmp", "a.txt"))-- /tmp/a.txt
    print(std.time.now())                -- 1740000000.123
"#).exec().unwrap();
```

## Per-module registration

For integration with `mlua-pkg` or custom loaders:

```rust
for (name, factory) in mlua_batteries::module_entries() {
    // name: "json", "env", ...
    // factory: fn(&Lua) -> LuaResult<LuaTable>
    let table = factory(&lua).unwrap();
}
```

## Sandboxing

The default configuration uses `Unrestricted` policies — Lua scripts can access any file, URL, and env var. For untrusted scripts, use `Sandboxed` (requires the `sandbox` feature):

```toml
[dependencies]
mlua-batteries = { version = "0.1", features = ["full"] }
```

```rust
use mlua::prelude::*;
use mlua_batteries::config::Config;
use mlua_batteries::policy::Sandboxed;

let lua = Lua::new();
let config = Config::builder()
    .path_policy(Sandboxed::new(["/app/data"]).unwrap().read_only())
    .max_walk_depth(50)
    .build()
    .expect("invalid config");

mlua_batteries::register_all_with(&lua, "std", config).unwrap();
```

### Policy layers

| Policy | Controls | Built-in options |
|--------|----------|-----------------|
| `PathPolicy` | Filesystem access | `Unrestricted`, `Sandboxed` (cap-std) |
| `HttpPolicy` | Outbound URLs | `Unrestricted` |
| `EnvPolicy` | Env var read/write | `Unrestricted` |
| `LlmPolicy` | LLM provider/model access | `Unrestricted` |

All policies are trait objects — implement the trait to create custom policies.

## LLM module

Built-in providers: **OpenAI**, **Anthropic**, **Ollama**. API keys are read from environment variables (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`).

```lua
local resp = std.llm.chat({
    provider = "openai",
    model    = "gpt-4o",
    prompt   = "Hello!",
    system   = "You are helpful.",
    max_tokens  = 1024,
    temperature = 0.7,
})
print(resp.content)

-- Batch parallel requests
local results = std.llm.batch({
    { provider = "ollama",  model = "llama3.2", prompt = "Q1" },
    { provider = "openai",  model = "gpt-4o",   prompt = "Q2" },
})
```

Custom providers can be registered via `mlua_batteries::llm::register_provider`.

## Configuration

`Config::builder()` exposes all tunable limits:

| Setting | Default | Description |
|---------|---------|-------------|
| `max_walk_depth` | 256 | Max directory depth for `fs.walk` |
| `max_walk_entries` | 10,000 | Max entries from `fs.walk` / `fs.glob` |
| `max_json_depth` | 128 | Max nesting depth for JSON |
| `http_timeout` | 30s | Default HTTP request timeout |
| `max_response_bytes` | 10 MiB | Max HTTP response body size |
| `max_sleep_secs` | 86,400 | Max `time.sleep` duration |
| `llm_default_timeout_secs` | 120 | Default LLM request timeout |
| `llm_max_response_bytes` | 10 MiB | Max LLM response body size |
| `llm_max_batch_concurrency` | 8 | Max threads for `llm.batch` |

## Platform support

**Unix only** (Linux, macOS). Windows is not a supported target.

All path arguments are UTF-8. Non-UTF-8 Lua strings are rejected at the `FromLua` boundary.

## MSRV

Rust **1.77** or later.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT License ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.
