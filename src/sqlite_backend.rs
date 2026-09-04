//! The SQLite stack the `std.sql` / `std.kv` bridges are built on.
//!
//! `libsqlite3-sys` declares `links = "sqlite3"`, so a build graph can hold
//! exactly one of its major versions — and cargo enforces that at dependency
//! *resolution* time, so a crate cannot offer several clusters behind
//! mutually exclusive features either.  Each published version of this crate
//! therefore tracks one cluster, the way `rusqlite-isle` does:
//!
//! | `mlua-batteries` | `rusqlite-isle` | `rusqlite` | `libsqlite3-sys` |
//! |---|---|---|---|
//! | 0.4 | 0.5 | 0.37 | 0.35 |
//!
//! The bridge code names the stack only through the two re-exports below, so
//! moving a release line onto another cluster is a two-line change in
//! `Cargo.toml` — the isle minors carry the same public API.
//!
//! Hosts wire `std.sql` / `std.kv` with an
//! [`AsyncIsle`](rusqlite_isle::AsyncIsle) built from these same versions.
//! Naming them through these re-exports keeps the host's types identical to
//! the bridge's without a second dependency declaration:
//!
//! ```rust,ignore
//! use mlua_batteries::sqlite_backend::rusqlite_isle::AsyncIsle;
//!
//! let (isle, driver) = AsyncIsle::open_in_memory(|_conn| Ok(())).await?;
//! mlua_batteries::sql::register(&lua, isle.clone())?;
//! ```

pub use {::isle037 as rusqlite_isle, ::rq037 as rusqlite};
