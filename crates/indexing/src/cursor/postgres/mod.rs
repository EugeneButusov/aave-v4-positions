//! The Postgres implementation of [`SyncStatusStore`](super::SyncStatusStore).
//!
//! One query over five columns, so the statement and the row stay in
//! `sync_status_store` rather than splitting as `aave-positions` does.

#[cfg(test)]
mod harness;
mod sync_status_store;

pub use sync_status_store::PostgresSyncStatusStore;
