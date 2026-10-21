use async_trait::async_trait;

use crate::chain::{BlockHeader, Error};

/// The three questions the loop and the detector put to a node.
///
/// Reading a log is not among them, and that is the seam rather than an
/// omission: the loop never reads one, so a fourth method here would be a
/// method every case driving the loop has to supply and nothing calls. It is
/// [`LogReader`](super::LogReader), injected into whoever does the reading.
#[async_trait]
pub trait ChainClient: Send + Sync {
    /// What the node says it is. Checked against the configured chain, because
    /// an indexer pointed at the wrong one produces plausible, wrong data.
    ///
    /// # Errors
    ///
    /// [`Error`] when the node cannot be reached or will not answer.
    async fn chain_id(&self) -> Result<u32, Error>;

    /// # Errors
    ///
    /// As [`chain_id`](Self::chain_id).
    async fn head_block_number(&self) -> Result<u64, Error>;

    /// A missing block is an error, never a `None`: every height asked for here
    /// is at or below a head this node has already reported.
    ///
    /// # Errors
    ///
    /// As [`chain_id`](Self::chain_id).
    async fn block_header(&self, number: u64) -> Result<BlockHeader, Error>;
}
