//! The row that says how far the indexer has got.

mod error;
mod postgres;
mod sync_status;
mod sync_status_store;

#[cfg(test)]
mod conformance;
#[cfg(test)]
mod harness;

pub use error::Error;
pub use postgres::PostgresSyncStatusStore;
pub use sync_status::SyncStatus;
pub use sync_status_store::SyncStatusStore;
