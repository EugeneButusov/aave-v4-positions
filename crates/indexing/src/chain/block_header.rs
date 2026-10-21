use alloy_primitives::B256;

/// A block, reduced to what deciding the shape of the chain needs.
///
/// No transactions: the window retains one of these per block for a whole
/// finality depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockHeader {
    pub number: u64,
    pub hash: B256,
    pub parent_hash: B256,

    /// Unix seconds, as the block reported them.
    pub timestamp: u64,
}
