//! The Postgres implementation of [`BlockHeaderStore`](super::BlockHeaderStore).
//!
//! Each port method is one statement, which is why this window lives in a
//! relational database rather than the event log's engine: upsert, prune and
//! truncate are ordinary things to ask of a table with a composite key, and
//! expensive ones to emulate on an append-only one.

mod block_header_store;
#[cfg(test)]
mod harness;

pub use block_header_store::PostgresBlockHeaderStore;
