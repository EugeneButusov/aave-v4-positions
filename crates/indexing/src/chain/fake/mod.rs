//! The fake implementations of [`ChainClient`](super::ChainClient).
//!
//! Headers are derived from a branch tag and a height rather than stored, so a
//! fork is self-consistent without a case enumerating either branch.

mod chain_client;

pub(crate) use chain_client::{ChainCall, FakeChain, ForkingChain};
