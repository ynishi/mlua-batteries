# mlua-batteries-sqlite-isle

SQLite bridge for [mlua-batteries](https://crates.io/crates/mlua-batteries): the `std.sql` and `std.kv` Lua modules, built on [rusqlite-isle](https://crates.io/crates/rusqlite-isle).

```toml
[dependencies]
mlua-batteries = { version = "0.5", features = ["task"] }
mlua-batteries-sqlite-isle = "0.5"     # see "Choosing a version"
tokio = { version = "1", features = ["rt", "macros"] }
```

```rust,ignore
use mlua_batteries_sqlite_isle::rusqlite_isle::AsyncIsle;

let lua = Lua::new();
mlua_batteries::register_all(&lua, "std")?;
mlua_batteries::task::register(&lua)?;

let (isle, _driver) = AsyncIsle::open_in_memory(|_conn| Ok(())).await?;
mlua_batteries_sqlite_isle::sql::register(&lua, isle.clone())?;
mlua_batteries_sqlite_isle::kv::register(&lua, isle).await?;
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
| `std.sql` | `query` / `exec` over a host-supplied isle, plus `std.sql.null` — the sentinel that keeps a SQL NULL column distinguishable from an absent one |
| `std.kv` | Namespace-scoped key-value store in a `__kv` table. Values are JSON; writes run under `BEGIN IMMEDIATE` |

Both are async-first and require a `tokio` current-thread runtime driving a `LocalSet`, the same runtime `mlua-batteries`' `task` feature needs.

## Relationship to `mlua-batteries-sqlite`

[`mlua-batteries-sqlite`](https://crates.io/crates/mlua-batteries-sqlite) is the default bridge: the host owns a `rusqlite::Connection` behind `Arc<Mutex<_>>` and statements run inside `tokio::task::spawn_blocking`, cancelled through the host's `InterruptHandle`. Reach for it unless you have a reason not to.

This crate is for hosts that already run SQLite on a `rusqlite-isle` connection thread and want `std.sql` / `std.kv` on that same `AsyncIsle` rather than a second, separately-locked connection. The Lua-side API is identical; only the host wiring differs.

Both sit on rusqlite 0.37, so a host may link both — the two bridges resolve to one `libsqlite3-sys` cluster.

## Wiring

The host owns the `AsyncIsle` and its `AsyncIsleDriver` (the driver shuts the connection thread down) and passes a clone of the handle. File path, `busy_timeout` and `journal_mode` are host-side concerns, applied in the isle's `init` closure or through `AsyncIsleBuilder::wal`. This crate does not open the database, does not read environment variables, and does not attempt to recover from a corrupt connection.

`std.sql` and `std.kv` are typically given **separate** isles: keeping KV scratch state out of the user database keeps their backup / WAL / page-cache lifecycles from colliding.

`kv::register` is `async` because the `__kv` table is created through the isle before `std.kv` is exposed. Hosts that would rather do it at open time can call `kv::init_schema` from the isle's `init` closure — the DDL is `CREATE TABLE IF NOT EXISTS`, so running it twice is harmless.

Statements run on the isle's connection thread, so no blocking call and no lock guard crosses an `.await`. Every query races the enclosing `task.scope` / `task.with_timeout` cancel token and the `SqlConfig` query timeout (5s by default): jobs are submitted with `spawn_call`, whose task cancels on drop, and the isle turns that into `sqlite3_interrupt`.

## Choosing a version

`libsqlite3-sys` declares `links = "sqlite3"`, so a build graph can hold exactly one of its major versions — and cargo enforces that while *resolving* dependencies, which means no crate can offer several clusters behind mutually exclusive features. Each release line therefore tracks one cluster, mirroring `rusqlite-isle`'s numbering:

| `mlua-batteries-sqlite-isle` | `rusqlite-isle` | `rusqlite` | `libsqlite3-sys` |
|---|---|---|---|
| `0.5` | 0.5 | 0.37 | 0.35 |

Pick the line whose cluster your other rusqlite-dependent crates already sit on. Name the host's types through `mlua_batteries_sqlite_isle::rusqlite` / `::rusqlite_isle` rather than declaring a second dependency that could drift onto another cluster.

This is also why the bridges are not part of `mlua-batteries` itself: the constraint belongs on this small crate, leaving the facade's version line free for its own features.

## Features

| feature | default | effect |
|---|---|---|
| `sqlite-bundled` | **on** | build and link SQLite from source (`rusqlite-isle/bundled`, which forwards to `rusqlite/bundled`). Turn it off with `default-features = false` to link the system library instead. |

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
