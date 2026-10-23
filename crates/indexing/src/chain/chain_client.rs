use async_trait::async_trait;
use units::ChainId;

use crate::chain::{BlockHeader, Error};

/// The three questions the loop and the detector put to a node. Reading a log
/// is [`LogReader`](super::LogReader)'s, for the reason given there.
#[async_trait]
pub trait ChainClient: Send + Sync {
    /// Checked against the configured chain: an indexer pointed at the wrong
    /// one produces plausible, wrong data.
    ///
    /// # Errors
    ///
    /// [`Error`] when the node cannot be reached or will not answer.
    async fn chain_id(&self) -> Result<ChainId, Error>;

    /// # Errors
    ///
    /// As [`chain_id`](Self::chain_id).
    async fn head_block_number(&self) -> Result<u64, Error>;

    /// Every height asked for is at or below a head this node has reported, so
    /// a missing block is an error rather than a `None`.
    ///
    /// # Errors
    ///
    /// As [`chain_id`](Self::chain_id).
    async fn block_header(&self, number: u64) -> Result<BlockHeader, Error>;
}
