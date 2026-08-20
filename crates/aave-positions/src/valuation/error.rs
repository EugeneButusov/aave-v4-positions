//! What the arithmetic refuses to answer.

use alloy_primitives::{I256, U256};

/// A valuation that cannot be computed.
///
/// [`Error::CheckpointAhead`] and [`Error::NegativePremium`] are the two the
/// contract reverts on, and they carry the same meaning here that a revert
/// carries there: not a number for the caller to handle, but a state the
/// protocol cannot be in.
///
/// [`Error::OutOfRange`] has no revert to correspond to, because Solidity's
/// checked arithmetic stops at the operation rather than reporting which one.
/// The claim is the same either way — every quantity these formulas touch is a
/// `uint256`.
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
    /// **No term name, deliberately.** Naming the intermediate meant eleven
    /// string literals restating the line each sat beside, read by nothing —
    /// the harness needs the variant, not the term, and neither does a caller.
    /// The nearest automatic substitute is one frame of `#[track_caller]`
    /// [`Location`](std::panic::Location), which reports a line inside libcore
    /// the moment the constructor is passed as a value rather than called, and
    /// which no test can assert against.
    #[error("an intermediate does not fit a uint256")]
    OutOfRange,
}

/// The operands behind [`Error::NegativePremium`], boxed.
///
/// Four 256-bit words inline would make every `Result` in the module 136 bytes
/// wide — `clippy::result_large_err` says so — to carry a branch the protocol
/// cannot reach. The operands are still worth having: the message is what an
/// operator reads when the fold has gone wrong, which is the only way here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegativePremium {
    /// How far below zero the premium came out. The message prints it with a
    /// minus in front, as the signed value it stands for.
    pub shortfall: U256,
    pub shares: U256,
    pub offset: I256,
    pub index: U256,
}
