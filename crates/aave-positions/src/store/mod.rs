//! The read side: one wallet's folded positions, valued.
//!
//! Nothing here writes: the fold is materialized views over the event ledger,
//! so a reorg repairs the projection without this module learning of it.

mod clickhouse;
#[cfg(test)]
mod conformance;
mod error;
#[cfg(test)]
mod fixtures;
mod position;
mod position_store;

pub use clickhouse::ClickHousePositionStore;
pub use error::Error;
pub use position::{Position, PositionAsset};
pub use position_store::{PositionKey, PositionPage, PositionQuery, PositionStore};
