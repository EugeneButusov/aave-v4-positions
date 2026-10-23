//! The Postgres implementation of [`BlockHeaderStore`](super::BlockHeaderStore).
//!
//! Each port method is one statement — which upsert, prune and truncate are
//! only on a table with a composite key, not on the event log's engine.

mod block_header_store;
#[cfg(test)]
mod harness;

pub use block_header_store::PostgresBlockHeaderStore;
