//! The row that says how far the indexer has got, read and written.

mod cursor_store;
mod error;
mod postgres;
mod sync_status;
mod sync_status_store;

#[cfg(test)]
pub(crate) mod conformance;
#[cfg(test)]
mod in_memory;

pub use cursor_store::{Cursor, CursorStore};
pub use error::Error;
pub use postgres::{PostgresCursorStore, PostgresSyncStatusStore};
pub use sync_status::SyncStatus;
pub use sync_status_store::SyncStatusStore;
