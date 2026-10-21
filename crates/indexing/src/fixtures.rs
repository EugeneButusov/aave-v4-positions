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
pub(crate) fn header(block: u64, branch: u8) -> BlockHeader {
    BlockHeader {
        number: block,
        hash: hash_of(branch, block),
        parent_hash: hash_of(branch, block.saturating_sub(1)),
        timestamp: 1_700_000_000 + block * 12,
    }
}
