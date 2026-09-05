# Changelog

All notable changes to `mlua-batteries-sqlite-isle` will be documented in this
file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0] - 2026-09-05

Current track: `rusqlite-isle 0.5` / `rusqlite 0.37` / `libsqlite3-sys 0.35`.

### Changed
- **Breaking**: `mlua` bumped from `0.11` to `0.12`; depends on
  `mlua-batteries 0.6`.  `mlua` types cross this crate's API, so consumers
  must move to `mlua 0.12` together.  No source changes were needed.
- Same SQLite cluster as 0.5.x (`rusqlite-isle 0.5` / `rusqlite 0.37`); the
  line-to-cluster table lives in `Cargo.toml` next to `version` and in the
  README.
- MSRV raised from 1.77 to 1.88 (required by `mlua 0.12`).

## [0.5.0] - 2026-09-04

Current track: `rusqlite-isle 0.5` / `rusqlite 0.37` / `libsqlite3-sys 0.35`.

First release. The code is the `rusqlite-isle` implementation of the
`std.sql` / `std.kv` bridges previously published as `mlua-batteries-sqlite`
0.5.0 — which itself came from `mlua-batteries` 0.4.0, where the bridges were
the `sql` / `kv` features. It moves here unchanged in behaviour so the default
`mlua-batteries-sqlite` crate can return to the sync model
(host-owned `Arc<Mutex<Connection>>` + `tokio::task::spawn_blocking`) in its
0.6.0. Only the crate and the module path differ:

```rust,ignore
// mlua-batteries-sqlite 0.5.0
mlua_batteries_sqlite::sql::register(&lua, isle.clone())?;
mlua_batteries_sqlite::kv::register(&lua, isle).await?;
// mlua-batteries-sqlite-isle 0.5.0
mlua_batteries_sqlite_isle::sql::register(&lua, isle.clone())?;
mlua_batteries_sqlite_isle::kv::register(&lua, isle).await?;
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
  `rusqlite-isle/bundled`; opt out with `default-features = false` to link
  the system SQLite.

### Notes

- **Version tracks.** `libsqlite3-sys` declares `links = "sqlite3"`, and cargo
  rejects a manifest naming two rusqlite clusters as optional dependencies
  even when only one is activated — the check runs during dependency
  resolution, not after feature activation. Serving several clusters
  therefore requires separate published version lines: `0.5` = rusqlite
  0.37. Lower lines (`0.3` / `0.2` for the 0.32 / 0.31 clusters) can be
  opened if a consumer needs them; none is published yet.
- The version starts at 0.5.0 rather than 0.1.0 so the first line matches
  the `rusqlite-isle 0.5` cluster it was built against.
- `mlua-batteries-sqlite` 0.6.0 sits on the same rusqlite 0.37 cluster, so a
  host may link both crates.
