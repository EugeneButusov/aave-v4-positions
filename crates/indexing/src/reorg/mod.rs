//! Deciding the shape of the chain, and retaining the evidence for it.
//!
//! Two ports that only ever appear together: a detector holds a window.

mod block_header_store;
mod error;
mod postgres;
mod reorg_detector;

#[cfg(test)]
pub(crate) mod conformance;

pub use block_header_store::BlockHeaderStore;
pub use error::Error;
pub use postgres::PostgresBlockHeaderStore;
pub use reorg_detector::{ReorgDetector, Verdict};
