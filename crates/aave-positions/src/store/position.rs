//! What a read hands back.

use alloy_primitives::{Address, I256, U256};

use units::ChainId;

use crate::valuation::Valuation;

/// What a reserve refers to, once the registry and the Hub have both been read.
///
/// `reserve_id` is a per-Spoke index and means nothing on its own (§1).
/// `AddReserve` supplies the Hub and asset id, the Hub's `AddAsset` the ERC-20
/// and its decimals — no Spoke event carries the token address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionAsset {
    pub asset_id: U256,
    pub hub: Address,
    pub underlying: Address,
    pub decimals: u8,
}

/// One user's stake in one reserve on one Spoke.
///
/// Shares are the stored truth and [`Position::value`] is derived: a share
/// balance is not a balance (§5), so the amounts are computed at read time and
/// carry the instant they were computed at.
///
/// [`Position::asset`] and [`Position::value`] are `None` together, and only
/// when the join has nothing to offer.
///
/// The share columns are signed although they cannot go negative on chain, so
/// that drift shows as a wrong number §9 can see rather than a refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    pub chain_id: ChainId,
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

    /// Net principal flow in asset units — cost basis, where the difference
    /// from [`Position::value`] is interest.
    pub net_supplied_amount: I256,
    pub net_borrowed_amount: I256,

    /// The user's own collateral flag, and only that. §12.1's `collateral`
    /// type also requires `collateralFactor > 0`, which needs config events the
    /// fold does not ingest — and five of the Main Spoke's fourteen reserves
    /// have `CF = 0`.
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
