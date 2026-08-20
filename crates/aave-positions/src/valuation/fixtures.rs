//! The vectors both halves of the valuation build on.
//!
//! Shared rather than duplicated: `asset()` is the state every formula reads,
//! and a second copy in the sibling would drift from this one silently.

use alloy_primitives::{I256, U256};

use super::RAY;
use super::asset::AssetState;
use super::position::PositionShares;

pub(super) const HOUR: u64 = 3_600;
pub(super) const YEAR: u64 = 365 * 24 * HOUR;
pub(super) const T0: u64 = 1_785_000_000;

/// 5% per annum, RAY-scaled, as `drawn_rate` arrives on `UpdateAsset`.
pub(super) const FIVE_PERCENT: u128 = 10u128.pow(27) / 20;

pub(super) fn five_percent() -> U256 {
    U256::from(FIVE_PERCENT)
}

pub(super) fn asset() -> AssetState {
    AssetState {
        liquidity: U256::from(1_000_000),
        added_shares: U256::from(1_000_000),
        drawn_shares: U256::from(400_000),
        swept: U256::ZERO,
        premium_shares: U256::ZERO,
        premium_offset_ray: I256::ZERO,
        deficit_ray: U256::ZERO,
        realized_fees: U256::ZERO,
        liquidity_fee: 0,
        checkpoint_index: RAY,
        drawn_rate: FIVE_PERCENT,
        checkpoint_at: T0,
    }
}

pub(super) fn position() -> PositionShares {
    PositionShares {
        supplied_shares: U256::ZERO,
        drawn_shares: U256::ZERO,
        premium_shares: U256::ZERO,
        premium_offset_ray: I256::ZERO,
    }
}
