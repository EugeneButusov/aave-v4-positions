//! The row that says how far the indexer has got.
//!
//! **The port is this level; the adapter is a directory down.** `postgres` is
//! the one implementation, and the harness that stands a scenario up for it
//! lives with it rather than beside the port it is proving.

mod error;
mod postgres;
mod sync_status;
mod sync_status_store;

#[cfg(test)]
mod conformance;

pub use error::Error;
pub use postgres::PostgresSyncStatusStore;
pub use sync_status::SyncStatus;
pub use sync_status_store::SyncStatusStore;
