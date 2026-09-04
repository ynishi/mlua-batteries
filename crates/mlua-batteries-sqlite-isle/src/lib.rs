//! SQLite bridge for [`mlua-batteries`](https://crates.io/crates/mlua-batteries):
//! the `std.sql` and `std.kv` Lua modules, built on
//! [`rusqlite-isle`](https://crates.io/crates/rusqlite-isle).
//!
//! ```rust,ignore
//! use mlua::prelude::*;
//! use mlua_batteries_sqlite_isle::rusqlite_isle::AsyncIsle;
//!
//! let lua = Lua::new();
//! mlua_batteries::register_all(&lua, "std")?;
//! mlua_batteries::task::register(&lua)?;
//!
//! let (isle, _driver) = AsyncIsle::open_in_memory(|_conn| Ok(())).await?;
//! mlua_batteries_sqlite_isle::sql::register(&lua, isle.clone())?;
//! mlua_batteries_sqlite_isle::kv::register(&lua, isle).await?;
//! // Lua: std.sql.query("SELECT 1 AS n")
//! ```
//!
//! [`mlua-batteries-sqlite`](https://crates.io/crates/mlua-batteries-sqlite)
//! is the default bridge: a host-owned `rusqlite::Connection` behind
//! `Arc<Mutex<_>>`, with statements in `tokio::task::spawn_blocking`. This
//! crate is the variant for hosts already running SQLite on a `rusqlite-isle`
//! connection thread — same Lua API, same rusqlite 0.37 cluster.
//!
//! # Why this is a separate crate
//!
//! `libsqlite3-sys` declares `links = "sqlite3"`, so a build graph can hold
//! exactly one of its major versions — and cargo enforces that while
//! *resolving* dependencies, which means no crate can offer several clusters
//! behind mutually exclusive features. Serving more than one cluster requires
//! more than one published version line.
//!
//! Putting that constraint on this small bridge keeps it off `mlua-batteries`
//! itself, whose version line stays free for its own features. Pick the line
//! whose cluster your other rusqlite-dependent crates already sit on:
//!
//! | `mlua-batteries-sqlite-isle` | `rusqlite-isle` | `rusqlite` | `libsqlite3-sys` |
//! |---|---|---|---|
//! | `0.5` | 0.5 | 0.37 | 0.35 |
//!
//! # Wiring
//!
//! The host owns the [`AsyncIsle`](rusqlite_isle::AsyncIsle) and its
//! `AsyncIsleDriver` (the driver shuts the connection thread down) and passes
//! a clone of the handle. File path, `busy_timeout` and `journal_mode` are
//! host-side concerns, applied in the isle's `init` closure or through
//! `AsyncIsleBuilder::wal`. This crate does not open the database, does not
//! read environment variables, and does not attempt to recover from a corrupt
//! connection.
//!
//! `std.sql` and `std.kv` are typically given **separate** isles: keeping KV
//! scratch state out of the user database keeps their backup / WAL /
//! page-cache lifecycles from colliding.

pub mod kv;
pub mod sql;
mod sqlite_backend;

/// The `rusqlite` this crate is built against.
///
/// Name the host's types through this re-export instead of declaring a second
/// `rusqlite` dependency, which could drift onto another cluster.
pub use sqlite_backend::rusqlite;

/// The `rusqlite-isle` this crate is built against — the source of the
/// `AsyncIsle` that [`sql::register`] and [`kv::register`] take.
pub use sqlite_backend::rusqlite_isle;

pub use sql::SqlConfig;
