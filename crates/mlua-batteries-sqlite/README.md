# mlua-batteries-sqlite

SQLite bridge for [mlua-batteries](https://crates.io/crates/mlua-batteries): the `std.sql` and `std.kv` Lua modules, built on [rusqlite](https://crates.io/crates/rusqlite) and `tokio::task::spawn_blocking`.

```toml
[dependencies]
mlua-batteries = { version = "0.5", features = ["task"] }
mlua-batteries-sqlite = "0.6"
tokio = { version = "1", features = ["rt", "macros"] }
```

```rust,ignore
use std::sync::{Arc, Mutex};
use mlua_batteries_sqlite::rusqlite::Connection;

let lua = Lua::new();
mlua_batteries::register_all(&lua, "std")?;
mlua_batteries::task::register(&lua)?;

let conn = Connection::open_in_memory()?;
let interrupt = Arc::new(conn.get_interrupt_handle());
let conn = Arc::new(Mutex::new(conn));
mlua_batteries_sqlite::sql::register(&lua, conn.clone(), interrupt.clone())?;
mlua_batteries_sqlite::kv::register(&lua, conn, interrupt)?;
```

```lua
std.sql.exec("CREATE TABLE t(x INTEGER, y TEXT)")
std.sql.exec("INSERT INTO t(x, y) VALUES(?, ?)", {42, "hello"})
local rows = std.sql.query("SELECT x, y FROM t WHERE x = ?", {42})

std.kv.set("session", "user", {id = 7, tags = std.json.array()})
local v = std.kv.get("session", "user")
```

## Modules

| Lua module | What it is |
|---|---|
| `std.sql` | `query` / `exec` over a host-supplied connection, plus `std.sql.null` — the sentinel that keeps a SQL NULL column distinguishable from an absent one |
| `std.kv` | Namespace-scoped key-value store in a `__kv` table. Values are JSON |

Both are async-first and require a `tokio` current-thread runtime driving a `LocalSet`, the same runtime `mlua-batteries`' `task` feature needs.

## Wiring

The host owns the `rusqlite::Connection` (file path, `busy_timeout` and `journal_mode` are host-side concerns) and its `InterruptHandle`, and passes them wrapped in `Arc<Mutex<_>>` / `Arc<_>`. This crate does not open the database, does not read environment variables, and does not attempt to recover from a corrupt connection.

`std.sql` and `std.kv` are typically given **separate** connections: keeping KV scratch state out of the user database keeps their backup / WAL / page-cache lifecycles from colliding.

Statements run inside `tokio::task::spawn_blocking`, with the mutex taken inside the blocking closure so no lock guard is held across an `.await`. Every query races the enclosing `task.scope` / `task.with_timeout` cancel token and the `SqlConfig` query timeout (5s by default); when either fires the bridge calls `sqlite3_interrupt` through the stored handle so the blocking thread returns and releases the connection.

## Choosing a version

`libsqlite3-sys` declares `links = "sqlite3"`, so a build graph can hold exactly one of its major versions — and cargo enforces that while *resolving* dependencies, which means no crate can offer several clusters behind mutually exclusive features. This crate sits on one cluster:

| `mlua-batteries-sqlite` | `rusqlite` | `libsqlite3-sys` |
|---|---|---|
| `0.6` | 0.37 | 0.35 |

Name the host's types through `mlua_batteries_sqlite::rusqlite` rather than declaring a second dependency that could drift onto another cluster.

Hosts that already run SQLite on a [rusqlite-isle](https://crates.io/crates/rusqlite-isle) connection thread want [`mlua-batteries-sqlite-isle`](https://crates.io/crates/mlua-batteries-sqlite-isle) instead — the same Lua API on an `AsyncIsle`, on the same rusqlite 0.37 cluster.

This is also why the bridges are not part of `mlua-batteries` itself: the SQLite dependency belongs on this small crate, leaving the facade free of a C library and its version line free for its own features.

## Features

| feature | default | effect |
|---|---|---|
| `sqlite-bundled` | **on** | build and link SQLite from source (`rusqlite/bundled`). Turn it off with `default-features = false` to link the system library instead. |

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
