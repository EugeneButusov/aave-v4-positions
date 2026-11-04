//! The loop, what it reports, and how a range reaches a processor.

mod cause;
mod dispatch;
mod indexer;
mod iteration;
pub(crate) mod metrics;
mod pacing;
mod pipeline;
mod status;
mod step;

#[cfg(test)]
mod harness;
#[cfg(test)]
mod tests;

pub use indexer::Indexer;
pub use iteration::Iteration;
pub use metrics::Instruments;
pub use pacing::SHUTDOWN_DRAIN;
pub use pipeline::{Options, Pipeline};
pub use status::{IndexerSnapshot, IndexerState, IndexerStatus};
