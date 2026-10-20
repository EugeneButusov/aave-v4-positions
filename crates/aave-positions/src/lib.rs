//! Aave v4 positions: what a wallet holds, and what it is worth.
//!
//! [`valuation`] is the arithmetic transcribed from the contracts, [`store`] is
//! the read side over the fold, [`scale`] is the units both use. Nothing here
//! writes; `bins/indexer` fills the ledger these views read.
//!
//! Every arithmetic operation is checked, which is what the crate-level lint
//! below buys: a product that leaves `uint256` becomes a [`valuation::Error`]
//! rather than a number.
#![deny(clippy::arithmetic_side_effects)]

pub mod scale;
pub mod store;
pub mod valuation;
