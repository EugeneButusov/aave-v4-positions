use alloy_primitives::{Address, B256, Bytes};
use async_trait::async_trait;

use crate::chain::Error;

/// One log, with everything needed to key it and nothing needed to decode it
/// twice.
///
/// `block_timestamp` is unconditional here although not every node returns it
/// on a log: an adapter that cannot get it from the log fills it from the
/// header, once per distinct block rather than once per log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLog {
    pub address: Address,

    /// `topics[0]` is the event signature hash, except on an anonymous event.
    pub topics: Vec<B256>,
    pub data: Bytes,
    pub block_number: u64,
    pub block_hash: B256,
    pub block_timestamp: u64,
    pub transaction_hash: B256,
    pub transaction_index: u64,

    /// Position within the **block**, not within the transaction, so
    /// `(block_number, log_index)` is unique.
    pub log_index: u64,
}

/// Which logs to read, over which blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogFilter {
    pub addresses: Vec<Address>,

    /// Empty means no constraint on the signature. A filter narrowed to an
    /// empty set of topics matches nothing, which is not the same request.
    pub topic0: Vec<B256>,

    /// Both ends inclusive.
    pub from_block: u64,
    pub to_block: u64,
}

/// Reading logs, which the loop does not do.
///
/// Its own port rather than a method on [`ChainClient`](super::ChainClient), so
/// what a collaborator can do is decided by what it holds: the loop cannot read
/// a log, and a processor cannot move the loop.
#[async_trait]
pub trait LogReader: Send + Sync {
    /// Every log in `filter`'s range, in no promised order beyond the one the
    /// node served them in.
    ///
    /// # Errors
    ///
    /// [`Error::RangeTooLarge`] when the provider refused the width, which the
    /// caller answers by narrowing; [`Error::Unavailable`] for anything else.
    async fn logs(&self, filter: &LogFilter) -> Result<Vec<RawLog>, Error>;
}
