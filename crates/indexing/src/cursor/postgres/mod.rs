//! The Postgres implementations of this module's two ports.
//!
//! One table, two adapters: the loop writes it through
//! [`CursorStore`](super::CursorStore) and the read API reads it through
//! [`SyncStatusStore`](super::SyncStatusStore).

mod cursor_store;
#[cfg(test)]
mod harness;
mod sync_status_store;

pub use cursor_store::PostgresCursorStore;
pub use sync_status_store::PostgresSyncStatusStore;
