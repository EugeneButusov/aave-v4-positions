//! What the chain is asked, and what it answers.
//!
//! No adapter for any of them: one links a provider and an HTTP transport,
//! which this crate may not — see the crate doc.

mod block_header;
mod chain_client;
mod error;
mod log_reader;

#[cfg(test)]
pub(crate) mod in_memory;

pub use block_header::BlockHeader;
pub use chain_client::ChainClient;
pub use error::Error;
pub use log_reader::{LogFilter, LogReader, RawLog};
