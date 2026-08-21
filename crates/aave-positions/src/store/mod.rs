//! The read side: one wallet's folded positions, valued.
//!
//! **Read-only by construction.** Nothing here writes. The fold is materialized
//! views over the event ledger, so positions advance because the indexer
//! appended events — there is no ingestion path here to keep in step with one,
//! and a reorg repairs the projection without this module learning of it.
//!
//! The split follows what each part answers to. `position` is what a caller
//! gets back, `query` is what it asks for, `position_store` is the ClickHouse
//! adapter that turns one into the other, and `error` is what it refuses with.

mod error;
#[cfg(test)]
mod fixtures;
mod position;
mod position_store;
mod query;

pub use error::Error;
pub use position::{Position, PositionAsset};
pub use position_store::ClickHousePositionStore;
pub use query::{PositionKey, PositionPage, PositionQuery};
