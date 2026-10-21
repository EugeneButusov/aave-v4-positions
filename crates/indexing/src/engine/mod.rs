//! The loop, what it reports, and how a range reaches a processor.

mod dispatch;
mod indexer;
mod iteration;
pub(crate) mod metrics;
mod status;

#[cfg(test)]
mod tests;

pub use indexer::{Indexer, Options, Pipeline, SHUTDOWN_DRAIN};
pub use iteration::Iteration;
pub use metrics::Instruments;
pub use status::{IndexerSnapshot, IndexerState, IndexerStatus};
