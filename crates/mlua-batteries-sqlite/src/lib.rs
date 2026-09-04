//! SQLite bridge for [`mlua-batteries`](https://crates.io/crates/mlua-batteries):
//! the `std.sql` and `std.kv` Lua modules, built on
//! [`rusqlite`](https://crates.io/crates/rusqlite) and
//! `tokio::task::spawn_blocking`.
//!
//! ```rust,ignore
//! use std::sync::{Arc, Mutex};
//! use mlua::prelude::*;
//! use mlua_batteries_sqlite::rusqlite::Connection;
//!
//! let lua = Lua::new();
//! mlua_batteries::register_all(&lua, "std")?;
//! mlua_batteries::task::register(&lua)?;
//!
//! let conn = Connection::open_in_memory()?;
//! let interrupt = Arc::new(conn.get_interrupt_handle());
//! let conn = Arc::new(Mutex::new(conn));
//! mlua_batteries_sqlite::sql::register(&lua, conn.clone(), interrupt.clone())?;
//! mlua_batteries_sqlite::kv::register(&lua, conn, interrupt)?;
//! // Lua: std.sql.query("SELECT 1 AS n")
//! ```
//!
//! # Why this is a separate crate
//!
//! `libsqlite3-sys` declares `links = "sqlite3"`, so a build graph can hold
//! exactly one of its major versions — and cargo enforces that while
//! *resolving* dependencies, which means no crate can offer several clusters
//! behind mutually exclusive features. Keeping the SQLite dependency on this
//! small bridge leaves `mlua-batteries` itself free of a C library, and
//! leaves its version line free for its own features. The `rusqlite` this
//! crate is built against is re-exported as [`rusqlite`]; name the host's
//! types through it.
//!
//! # Wiring
//!
//! The host owns the [`rusqlite::Connection`] (file path, `busy_timeout` and
//! `journal_mode` are host-side concerns) and its
//! [`rusqlite::InterruptHandle`], and passes them wrapped in `Arc<Mutex<_>>`
//! / `Arc<_>`. This crate does not open the database, does not read
//! environment variables, and does not attempt to recover from a corrupt
//! connection.
//!
//! Statements run inside `tokio::task::spawn_blocking`, with the mutex taken
//! inside the blocking closure so no lock guard is held across an `.await`.
//! Every query races the enclosing `task.scope` / `task.with_timeout` cancel
//! token and the [`SqlConfig`] query timeout; when either fires the bridge
//! calls `sqlite3_interrupt` through the stored handle so the blocking thread
//! returns and releases the connection.
//!
//! `std.sql` and `std.kv` are typically given **separate** connections:
//! keeping KV scratch state out of the user database keeps their backup /
//! WAL / page-cache lifecycles from colliding.

pub mod kv;
pub mod sql;
mod sqlite_backend;

/// The `rusqlite` this crate is built against.
///
/// Name the host's types through this re-export instead of declaring a second
/// `rusqlite` dependency, which could drift onto another cluster.
pub use sqlite_backend::rusqlite;

pub use sql::SqlConfig;
