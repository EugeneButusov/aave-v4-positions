//! Deciding the shape of the chain, and retaining the evidence for it.
//!
//! Two ports that only ever appear together: a detector holds a window, and the
//! window is durable because `Cursor::last_hash` alone cannot locate a fork
//! point once the resume block has been reorged out.

mod block_header_store;
mod error;
mod hash_chain;
mod postgres;
mod reorg_detector;

#[cfg(test)]
pub(crate) mod conformance;
#[cfg(test)]
mod harness;
#[cfg(test)]
mod tests;

pub use block_header_store::BlockHeaderStore;
pub use error::Error;
pub use hash_chain::HashChainReorgDetector;
pub use postgres::PostgresBlockHeaderStore;
pub use reorg_detector::{ReorgDetector, Verdict};
