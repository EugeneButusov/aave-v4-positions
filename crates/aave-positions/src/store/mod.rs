//! The read side: one wallet's folded positions, valued.
//!
//! **Read-only by construction.** Nothing here writes. The fold is materialized
//! views over the event ledger, so positions advance because the indexer
//! appended events — there is no ingestion path here to keep in step with one,
//! and a reorg repairs the projection without this module learning of it.
//!
//! **The port is this level; the adapter is a directory down.** `position_store`
//! is what a caller asks for, what it gets back and the method between them,
//! `position` is the row it sees, and `error` is what a read refuses with — none
//! of which names a database. `clickhouse` is the one implementation, and
//! everything that knows about `toString`, `LEFT JOIN` or a `{name:Type}`
//! parameter lives inside it.

mod clickhouse;
mod error;
mod position;
mod position_store;

pub use clickhouse::ClickHousePositionStore;
pub use error::Error;
pub use position::{Position, PositionAsset};
pub use position_store::{PositionKey, PositionPage, PositionQuery, PositionStore};
