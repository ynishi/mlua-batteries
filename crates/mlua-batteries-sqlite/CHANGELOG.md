# Changelog

All notable changes to `mlua-batteries-sqlite` will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
