//! The ClickHouse client this repository uses.
//!
//! Connectivity only. Anything that knows what a *migration* is lives with the
//! binary that applies them — a crate named for a database should not also be
//! the place migration semantics are decided.

mod client;

/// The driver, so a caller naming a `Client` or an `Error` needs one dependency
/// and cannot end up on a different version than this crate was built against.
pub use clickhouse;
pub use client::{Config, client};
