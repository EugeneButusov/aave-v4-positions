//! The identifiers and instants every crate here agrees on.
//!
//! A width is a decision about what the chain can hand us, so it is made once
//! here rather than at each boundary that reads one.

mod chain_id;

pub use chain_id::{ChainId, NotAChainId};
