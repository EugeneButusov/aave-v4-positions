//! What the arithmetic refuses to answer.

use alloy_primitives::{I256, U256};

/// A valuation that cannot be computed.
///
/// Two of these are the TypeScript's `RangeError`s, kept to the same message
/// text so the differential harness can compare error conditions and so the
/// strings still grep. The third has no counterpart, and it is the one place
/// this is deliberately not a transcription: `bigint` has no width, so where
/// this returns [`Error::OutOfRange`] the TypeScript returns a number outside
/// `uint256` and carries on. Every quantity here is a `uint256` on chain, so
/// that number was never a state the protocol could be in — but saying so is
/// new, and it is what the widths bought.
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

    /// A caller passed a zero denominator, which `bigint` division rejects too.
    #[error("division by zero")]
    DivideByZero,

    /// An intermediate left `uint256`, in either direction.
    #[error("{what} does not fit a uint256")]
    OutOfRange { what: &'static str },
}

/// The operands behind [`Error::NegativePremium`], boxed.
///
/// Four 256-bit words inline would make every `Result` in the module 136 bytes
/// wide — `clippy::result_large_err` says so — to carry a branch the protocol
/// cannot reach. The operands are still worth having: the message is what an
/// operator reads when the fold has gone wrong, which is the only way here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegativePremium {
    /// How far below zero. The TypeScript prints this with a minus in front.
    pub shortfall: U256,
    pub shares: U256,
    pub offset: I256,
    pub index: U256,
}
