//! The ClickHouse client this repository uses.
//!
//! Connectivity only. Anything that knows what a *migration* is lives with the
//! binary that applies them — a crate named for a database should not also be
//! the place migration semantics are decided.

mod client;

pub use client::{Config, client};
