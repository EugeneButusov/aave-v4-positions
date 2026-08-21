//! What a read hands back.

use alloy_primitives::{Address, I256, U256};

use crate::valuation::Valuation;

/// What a reserve refers to, once the registry and the Hub have both been read.
///
/// `reserve_id` is a per-Spoke index and means nothing on its own (§1). This is
/// the resolved identity: `AddReserve` supplies the Hub and its asset id, and
/// the Hub's `AddAsset` supplies the ERC-20 and its decimals — no Spoke event
/// carries the token address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionAsset {
    pub asset_id: U256,
    pub hub: Address,
    pub underlying: Address,
    pub decimals: u8,
}

/// One user's stake in one reserve on one Spoke.
///
/// **Shares are the stored truth; [`Position::value`] is derived.** Between
/// events the Hub's index accrues and emits nothing, so a share balance is not
/// a balance (§5) — the amounts are computed at read time from the Hub asset
/// fold and carry the instant they were computed at.
///
/// [`Position::asset`] and [`Position::value`] are **`None` together**, and
/// only when the join has nothing to offer: a reserve the registry has not
/// seen, or a Hub asset with no interest checkpoint yet. Reporting zero there
/// would be a number the caller cannot distinguish from a real one.
///
/// **The share columns are signed, and that is not a widening for convenience.**
/// Each is a sum of `Int256` deltas and cannot go negative on chain, so a
/// negative one is drift — and §9 catches it by seeing it. Reading them as
/// `U256` would turn the wrong number that reveals a bug into a refusal that
/// hides which position produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    pub chain_id: u32,
    /// Lower-case. §12.1 keys on the user, never the caller that routed the call.
    pub user: Address,
    /// The Spoke this position lives on. Two Spokes are two independent positions.
    pub spoke: Address,
    /// The Spoke's own key for the reserve. Resolved to a token by [`Position::asset`].
    pub reserve_id: U256,

    pub supplied_shares: I256,
    pub drawn_shares: I256,
    pub premium_shares: I256,
    pub premium_offset_ray: I256,

    /// Net principal flow in asset units — what was put in less what came out.
    ///
    /// Kept beside [`Position::value`] rather than replaced by it, because the
    /// two answer different questions: this one is cost basis, and the
    /// difference between them is interest.
    pub net_supplied_amount: I256,
    pub net_borrowed_amount: I256,

    /// The user's own collateral flag, and only that.
    ///
    /// §12.1's `collateral` type additionally requires `collateralFactor > 0`
    /// under the user's pinned `dynamicConfigKey`, which needs config events
    /// the fold does not ingest. Five of the Main Spoke's fourteen reserves
    /// have `CF = 0`, so this flag alone overstates collateral for those.
    pub using_as_collateral: bool,

    /// Ledger rows folded into this position. Zero shares with a non-zero count
    /// is a closed position.
    pub events: i32,

    /// `None` when the registry has not seen this reserve.
    pub asset: Option<PositionAsset>,
    /// `None` when [`Position::asset`] is, or when the Hub asset has no
    /// checkpoint yet.
    pub value: Option<Valuation>,
}
