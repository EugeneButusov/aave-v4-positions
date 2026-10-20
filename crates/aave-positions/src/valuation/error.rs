//! What the arithmetic refuses to answer.

use alloy_primitives::{I256, U256};

/// A valuation that cannot be computed.
///
/// [`Error::CheckpointAhead`] and [`Error::NegativePremium`] are the two the
/// contract reverts on, and mean here what a revert means there: a state the
/// protocol cannot be in. [`Error::OutOfRange`] has no revert to correspond
/// to, because Solidity's checked arithmetic stops at the operation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The valuation time is before the checkpoint it would extrapolate from.
    #[error("checkpoint is {seconds}s ahead of the valuation time")]
    CheckpointAhead { seconds: u64 },

    /// `premiumShares * drawnIndex` came out below `premiumOffsetRay`.
    #[error(
        "premium is negative (-{}): shares {}, offset {}, index {}",
        .0.shortfall, .0.shares, .0.offset, .0.index
    )]
    NegativePremium(Box<NegativePremium>),

    /// A caller passed a zero denominator. Unreachable from inside this
    /// crate, where every denominator is a nonzero constant or
    /// `added_shares + VIRTUAL`.
    #[error("division by zero")]
    DivideByZero,

    /// An intermediate left `uint256`, in either direction.
    ///
    /// No term name: it meant eleven string literals restating the line each
    /// sat beside, and `#[track_caller]` reports a line inside libcore the
    /// moment the constructor is passed as a value rather than called.
    #[error("an intermediate does not fit a uint256")]
    OutOfRange,
}

/// The operands behind [`Error::NegativePremium`], boxed because four 256-bit
/// words inline would make every `Result` in the module 136 bytes wide
/// (`clippy::result_large_err`) for a branch the protocol cannot reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegativePremium {
    /// How far below zero the premium came out; the message prints the minus.
    pub shortfall: U256,
    pub shares: U256,
    pub offset: I256,
    pub index: U256,
}
