//! The SQLite stack this crate is built on, in one place.
//!
//! `sql` and `kv` name `rusqlite` only through this re-export, so moving the
//! crate onto another rusqlite cluster is an edit of the one dependency line
//! in `Cargo.toml` plus the version.

pub use ::rusqlite;
