//! The Postgres implementation of [`SyncStatusStore`](super::SyncStatusStore).
//!
//! `sync_status_store` is the statement, the row it comes back as and the read
//! that joins them — one query over five columns, where `aave-positions` splits
//! `sql` and `row` out because its statement is neither. `harness` stands up the
//! migrated schema its specs read from.
//!
//! The specs split the same way. Every case here is the port's, in
//! `cursor::conformance`, run through this adapter's `Harness`; nothing about
//! this implementation needs one of its own yet.

#[cfg(test)]
mod harness;
mod sync_status_store;

pub use sync_status_store::PostgresSyncStatusStore;
