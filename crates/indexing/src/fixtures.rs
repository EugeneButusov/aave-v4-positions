//! The chain ids, the hashes and the headers every suite starts from.
//!
//! Nothing here writes anything anywhere: these are shapes, and they read the
//! same whichever implementation is under test. What puts them in front of one
//! is that implementation's harness.

use alloy_primitives::B256;

use crate::chain::BlockHeader;

pub(crate) const CHAIN_ID: u32 = 1;
pub(crate) const OTHER_CHAIN: u32 = 8453;

/// A hash that says which branch it is on and which block it is.
///
/// The bytes it means rather than a padded decimal string: a branch tag, then
/// the height big-endian, so two branches at one height differ and a case can
/// write `hash_of(b'b', 500)` to name the fork's version of block 500.
pub(crate) fn hash_of(branch: u8, block: u64) -> B256 {
    let mut bytes = [0_u8; 32];
    bytes[0] = branch;
    bytes[24..].copy_from_slice(&block.to_be_bytes());

    B256::from(bytes)
}

/// A header on `branch` at `block`, its parent the same branch below it.
///
/// Genesis has no parent, so its `parent_hash` is zero rather than its own
/// hash — the chain double composes the two sides separately and they have to
/// agree about that block.
pub(crate) fn header(block: u64, branch: u8) -> BlockHeader {
    BlockHeader {
        number: block,
        hash: hash_of(branch, block),
        parent_hash: parent_of(block, branch),
        timestamp: timestamp_of(block),
    }
}

/// The hash a block on `branch` names as its parent.
pub(crate) fn parent_of(block: u64, branch: u8) -> B256 {
    block
        .checked_sub(1)
        .map_or(B256::ZERO, |below| hash_of(branch, below))
}

/// Twelve seconds a block, from a fixed epoch, so a header is reproducible.
pub(crate) fn timestamp_of(block: u64) -> u64 {
    1_700_000_000 + block * 12
}
