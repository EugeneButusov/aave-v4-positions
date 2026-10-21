use alloy_primitives::B256;

/// A block, reduced to what deciding the shape of the chain needs.
///
/// Four fields and no transactions: the window retains one of these per block
/// for a whole finality depth, and a header is the only part of a block that
/// answers "does this follow the one below it".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockHeader {
    pub number: u64,
    pub hash: B256,
    pub parent_hash: B256,

    /// Unix seconds, as the block reported them.
    pub timestamp: u64,
}
