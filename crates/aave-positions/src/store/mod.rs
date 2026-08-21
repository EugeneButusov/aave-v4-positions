//! The read side: one wallet's folded positions, valued.
//!
//! **Read-only by construction.** Nothing here writes. The fold is materialized
//! views over the event ledger, so positions advance because the indexer
//! appended events — there is no ingestion path here to keep in step with one,
//! and a reorg repairs the projection without this module learning of it.
//!
//! The split follows what each part answers to. `position_store` is the port —
//! what a caller asks for, what it gets back, and the method between them —
//! and `clickhouse_position_store` is its one implementation. Under that,
//! `position` is the row a caller sees, `sql` is the statement, `row` is what
//! the server sends and what it means, and `error` is what a read refuses
//! with. The file names are the TypeScript's, so the two are diffable.

mod clickhouse_position_store;
mod error;
#[cfg(test)]
mod fixtures;
mod position;
mod position_store;
mod row;
mod sql;

pub use clickhouse_position_store::ClickHousePositionStore;
pub use error::Error;
pub use position::{Position, PositionAsset};
pub use position_store::{PositionKey, PositionPage, PositionQuery, PositionStore};
