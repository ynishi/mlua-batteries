//! The SQLite stack this crate is built on, in one place.
//!
//! `sql` and `kv` name `rusqlite` / `rusqlite_isle` only through these two
//! re-exports, so moving a release line onto another rusqlite cluster is an
//! edit of the two dependency lines in `Cargo.toml` plus the version — the
//! isle minors carry the same public API. See the track table in
//! [the crate docs](crate).

pub use ::rusqlite;
pub use ::rusqlite_isle;
