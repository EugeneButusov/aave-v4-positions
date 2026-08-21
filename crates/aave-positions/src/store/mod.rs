//! The read side: one wallet's folded positions, valued.
//!
//! **Read-only by construction.** Nothing here writes. The fold is materialized
//! views over the event ledger, so positions advance because the indexer
//! appended events — there is no ingestion path here to keep in step with one,
//! and a reorg repairs the projection without this module learning of it.
//!
//! The split follows what each part answers to. `query` is what a caller asks
//! for and `position` is what it gets back; `sql` is the statement between
//! them, `row` is what the server sends and what it means, `position_store` is
//! the adapter that runs one and returns the other, and `error` is what it
//! refuses with.

mod error;
#[cfg(test)]
mod fixtures;
mod position;
mod position_store;
mod query;
mod row;
mod sql;

pub use error::Error;
pub use position::{Position, PositionAsset};
pub use position_store::ClickHousePositionStore;
pub use query::{PositionKey, PositionPage, PositionQuery};
