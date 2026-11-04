//! What a range is handed to.

mod block_processor;

#[cfg(test)]
pub(crate) mod scripted;

pub use block_processor::{BlockProcessor, Failure, Outcome};
