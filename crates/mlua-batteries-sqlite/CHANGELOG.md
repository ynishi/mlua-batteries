# Changelog

All notable changes to `mlua-batteries-sqlite` will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0] - 2026-09-04

Current track: `rusqlite 0.37` / `libsqlite3-sys 0.35`.

### Changed

- **BREAKING** — `std.sql` / `std.kv` are back on the execution model they
  had in `mlua-batteries` 0.3.x: a host-owned `rusqlite::Connection` shared
  through `Arc<Mutex<_>>`, statements run inside
  `tokio::task::spawn_blocking`, and cancellation via the host's
  `InterruptHandle`. The `rusqlite-isle` rewrite (0.5.0, inherited from
  `mlua-batteries` 0.4.0) is dropped; it put a second thread-confined actor
  under the Lua VM's own runtime and tied this crate's version line to the
  isle's. The Lua-side API is unchanged; the host wiring reverts:
  - `sql::register(lua, isle)` → `sql::register(lua, conn, interrupt)`, with
    `conn: Arc<Mutex<Connection>>` and `interrupt: Arc<InterruptHandle>`.
    Same for `register_with`.
  - `kv::register` / `kv::register_with` are **synchronous** again; the
    `__kv` table is created on the supplied connection at registration.
    `kv::init_schema` is no longer public.
  - `std.kv` writes run as plain statements again (the 0.5.0
    `BEGIN IMMEDIATE` wrapper went with the isle).
- The `rusqlite_isle` re-export is gone. `rusqlite` is still re-exported so
  hosts can name the bridge's exact version without a second dependency.
- The version no longer mirrors `rusqlite-isle`'s minor. This crate sits on
  one rusqlite cluster, decided by its single `rusqlite` dependency line;
  see the README table.

### Removed

- `rusqlite-isle` dependency.

## [0.5.0] - 2026-09-04

Current track: `rusqlite-isle 0.5` / `rusqlite 0.37` / `libsqlite3-sys 0.35`.

First release. The `std.sql` / `std.kv` bridges were `mlua-batteries`'
`sql` / `kv` features up to that crate's 0.4.0 and move here unchanged in
behaviour; only the crate and the module path differ:

```rust,ignore
// mlua-batteries 0.4.0
mlua_batteries::sql::register(&lua, isle.clone())?;
mlua_batteries::kv::register(&lua, isle).await?;
// mlua-batteries-sqlite 0.5.0
mlua_batteries_sqlite::sql::register(&lua, isle.clone())?;
mlua_batteries_sqlite::kv::register(&lua, isle).await?;
```

### Added

- `std.sql`: `query` / `exec` / `null`, running on the connection thread of a
  host-supplied `rusqlite_isle::AsyncIsle`.
- `std.kv`: namespace-scoped key-value store over a `__kv` table, with
  `get` / `set` / `delete` / `list`. Writes run inside `BEGIN IMMEDIATE`.
  `kv::register` is `async`; `kv::init_schema` is public for hosts that
  prefer to create the table in the isle's `init` closure.
- `rusqlite` / `rusqlite_isle` re-exports, so hosts can name the exact
  versions this crate is built against without a second dependency that could
  drift onto another `libsqlite3-sys` cluster.
- `sqlite-bundled` feature, **enabled by default**, forwarding to
  `rusqlite/bundled`; opt out with `default-features = false` to link the
  system SQLite.

### Notes

- **Version tracks.** `libsqlite3-sys` declares `links = "sqlite3"`, and cargo
  rejects a manifest naming two rusqlite clusters as optional dependencies
  even when only one is activated — the check runs during dependency
  resolution, not after feature activation. Serving several clusters
  therefore requires separate published version lines, whose minors mirror
  `rusqlite-isle`'s: `0.5` = rusqlite 0.37, and lower lines are available for
  the 0.32 / 0.31 clusters when a consumer needs them.
- The version starts at 0.5.0 rather than 0.1.0 for that reason: the minor is
  the track, so it has to line up with `rusqlite-isle`'s from the first
  release.
