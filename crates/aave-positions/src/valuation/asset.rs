//! `AssetLogic` and `SharesMath`, transcribed from `aave/aave-v4` at commit
//! `2524fe4`. `AssetLogic` declares `using AssetLogic for IHub.Asset`, so these
//! are methods on chain too.

use alloy_primitives::{I256, U256, uint};

use super::Error;
use super::math::{
    from_ray_up, linear_interest, mul_div_down, percent_mul_down, premium_ray, ray_mul_up,
};

/// The Hub asset state one valuation reads.
///
/// A narrowed view of the `hub_assets_as_of` row, so a change to the row shape
/// cannot silently alter a formula. The summed columns are non-negative in
/// aggregate; `premium_offset_ray` stays signed because it genuinely is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AssetState {
    pub(crate) liquidity: U256,
    pub(crate) added_shares: U256,
    pub(crate) drawn_shares: U256,
    pub(crate) swept: U256,
    pub(crate) premium_shares: U256,
    pub(crate) premium_offset_ray: I256,
    pub(crate) deficit_ray: U256,
    pub(crate) realized_fees: U256,
    /// Basis points, as `UpdateAssetConfig` carries it — `Nullable(UInt16)`.
    pub(crate) liquidity_fee: u16,
    /// The last `UpdateAsset`'s index — the checkpoint, not the current value.
    pub(crate) checkpoint_index: U256,
    /// `uint96` on chain, which is what makes `linear_interest` total.
    pub(crate) drawn_rate: u128,
    /// Unix seconds. `accrue()` sets it to the checkpoint block's timestamp.
    pub(crate) checkpoint_at: u64,
}

/// `SharesMath.VIRTUAL_ASSETS` and `VIRTUAL_SHARES`, both 1e6.
const VIRTUAL: U256 = uint!(1_000_000_U256);

impl AssetState {
    /// `AssetLogic.getDrawnIndex`, extrapolated to an arbitrary time.
    ///
    /// Exact rather than a model because the Hub emits the settled index on
    /// every accrual (§5.3), so this applies linear interest to an
    /// authoritative checkpoint.
    ///
    /// Both short-circuits are the contract's. The second is on the *asset's*
    /// totals, not the user's, so a user with debt in an idle asset does not
    /// accrue; skipping either inflates every debt in that asset.
    pub(super) fn drawn_index_at(&self, at: u64) -> Result<U256, Error> {
        if self.checkpoint_at == at {
            return Ok(self.checkpoint_index);
        }
        if self.drawn_shares.is_zero() && self.premium_shares.is_zero() {
            return Ok(self.checkpoint_index);
        }

        ray_mul_up(
            self.checkpoint_index,
            linear_interest(self.drawn_rate, self.checkpoint_at, at)?,
        )
    }

    /// `AssetLogic._calculateAggregatedOwedRay` — everything the asset is owed,
    /// RAY-scaled.
    fn aggregated_owed_ray(&self, drawn_index: U256) -> Result<U256, Error> {
        let drawn = self
            .drawn_shares
            .checked_mul(drawn_index)
            .ok_or(Error::OutOfRange)?;

        drawn
            .checked_add(premium_ray(
                self.premium_shares,
                self.premium_offset_ray,
                drawn_index,
            )?)
            .and_then(|owed| owed.checked_add(self.deficit_ray))
            .ok_or(Error::OutOfRange)
    }

    /// `AssetLogic.getUnrealizedFees` — the protocol's cut of interest accrued
    /// since the checkpoint.
    ///
    /// A difference of two *separately rounded* values, not
    /// `from_ray_up(after - before)`: the two differ by a wei whenever both
    /// have a remainder.
    fn unrealized_fees(&self, drawn_index: U256) -> Result<U256, Error> {
        if self.checkpoint_index == drawn_index || self.liquidity_fee == 0 {
            return Ok(U256::ZERO);
        }

        let after = from_ray_up(self.aggregated_owed_ray(drawn_index)?);
        let before = from_ray_up(self.aggregated_owed_ray(self.checkpoint_index)?);

        percent_mul_down(
            after.checked_sub(before).ok_or(Error::OutOfRange)?,
            U256::from(self.liquidity_fee),
        )
    }

    /// `AssetLogic.totalAddedAssets` — what the whole supply side is worth.
    ///
    /// Depends on `drawn_index` and therefore on time: suppliers are paid out
    /// of accrued debt (§5.2).
    fn total_added_assets(&self, drawn_index: U256) -> Result<U256, Error> {
        let owed = from_ray_up(self.aggregated_owed_ray(drawn_index)?);
        let fees = self.unrealized_fees(drawn_index)?;

        self.liquidity
            .checked_add(self.swept)
            .and_then(|total| total.checked_add(owed))
            .and_then(|total| total.checked_sub(self.realized_fees))
            .and_then(|total| total.checked_sub(fees))
            .ok_or(Error::OutOfRange)
    }

    /// `SharesMath.toAssetsDown`, via `previewRemoveByShares` — the supply side.
    ///
    /// Not an index: the ERC-4626 virtual assets and shares sit inside the
    /// division, so they cannot be factored out. Rounds down, where the debt
    /// side rounds up.
    pub(super) fn supplied_assets(&self, shares: U256, drawn_index: U256) -> Result<U256, Error> {
        let assets = self
            .total_added_assets(drawn_index)?
            .checked_add(VIRTUAL)
            .ok_or(Error::OutOfRange)?;
        let shares_out = self
            .added_shares
            .checked_add(VIRTUAL)
            .ok_or(Error::OutOfRange)?;

        mul_div_down(shares, assets, shares_out)
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use super::*;
    use crate::valuation::RAY;
    use crate::valuation::fixtures::*;

    mod the_drawn_index {
        use super::*;

        #[test]
        fn applies_linear_interest_from_the_checkpoint() {
            assert_eq!(asset().drawn_index_at(T0 + YEAR), Ok(RAY + five_percent()));
        }

        #[test]
        fn compounds_only_where_a_checkpoint_landed() {
            // Two years in one step is 10%, not 10.25% (§5.1).
            assert_eq!(
                asset().drawn_index_at(T0 + 2 * YEAR),
                Ok(RAY + U256::from(2) * five_percent())
            );
        }

        #[test]
        fn does_not_move_at_the_checkpoint_itself() {
            assert_eq!(asset().drawn_index_at(T0), Ok(RAY));
        }

        #[test]
        fn does_not_accrue_on_an_asset_that_owes_nothing() {
            let idle = AssetState {
                drawn_shares: U256::ZERO,
                premium_shares: U256::ZERO,
                ..asset()
            };

            assert_eq!(idle.drawn_index_at(T0 + YEAR), Ok(RAY));
        }

        #[test]
        fn still_accrues_when_only_the_premium_is_outstanding() {
            // Both must be zero, not either.
            let premium_only = AssetState {
                drawn_shares: U256::ZERO,
                premium_shares: U256::from(1),
                ..asset()
            };

            assert!(premium_only.drawn_index_at(T0 + YEAR).unwrap() > RAY);
        }

        #[test]
        fn the_checkpoint_short_circuit_is_load_bearing_at_the_top_of_the_range() {
            // Below `rayMulUp`'s guard the branch is invisible — the else-branch
            // is `ray_mul_up(index, RAY)`, which is `index`. The TypeScript
            // suite survived deleting it; past the guard it is the difference
            // between an answer and none.
            for index in [U256::ZERO, U256::from(1), RAY, U256::MAX / RAY] {
                let state = AssetState {
                    checkpoint_index: index,
                    ..asset()
                };

                assert_eq!(state.drawn_index_at(T0), Ok(index));
                assert_eq!(
                    ray_mul_up(index, linear_interest(state.drawn_rate, T0, T0).unwrap()),
                    Ok(index)
                );
            }

            let past_the_guard = AssetState {
                checkpoint_index: U256::MAX,
                ..asset()
            };

            assert_eq!(past_the_guard.drawn_index_at(T0), Ok(U256::MAX));
            assert!(ray_mul_up(U256::MAX, RAY).is_err());
        }

        #[test]
        fn refuses_a_checkpoint_in_the_future() {
            assert_eq!(
                asset().drawn_index_at(T0 - 1),
                Err(Error::CheckpointAhead { seconds: 1 })
            );
        }
    }

    mod the_supply_side {
        use super::*;

        #[test]
        fn pads_the_ratio_with_virtual_assets_and_shares() {
            let state = AssetState {
                liquidity: U256::ZERO,
                added_shares: U256::ZERO,
                drawn_shares: U256::ZERO,
                premium_shares: U256::ZERO,
                ..asset()
            };

            // An empty asset: shares redeem 1:1 against the padding alone.
            assert_eq!(
                state.supplied_assets(U256::from(500), RAY),
                Ok(U256::from(500))
            );
            assert_eq!(VIRTUAL, U256::from(1_000_000));
        }

        #[test]
        fn rounds_down_where_debt_rounds_up() {
            let state = AssetState {
                liquidity: U256::from(1),
                added_shares: U256::from(3),
                drawn_shares: U256::ZERO,
                premium_shares: U256::ZERO,
                ..asset()
            };

            // (1 * (1 + 1e6)) / (3 + 1e6) floors to 0 rather than rounding to 1.
            assert_eq!(state.supplied_assets(U256::from(1), RAY), Ok(U256::ZERO));
        }

        #[test]
        fn counts_swept_liquidity_and_the_amount_owed_and_subtracts_settled_fees() {
            let state = AssetState {
                liquidity: U256::from(1_000),
                swept: U256::from(500),
                drawn_shares: U256::from(2_000),
                realized_fees: U256::from(300),
                premium_shares: U256::ZERO,
                premium_offset_ray: I256::ZERO,
                ..asset()
            };

            // 1000 + 500 + 2000 − 300.
            assert_eq!(state.total_added_assets(RAY), Ok(U256::from(3_200)));
        }

        #[test]
        fn carries_bad_debt_as_if_still_owed() {
            // §12.3: a deficit stays inside aggregatedOwedRay until eliminated,
            // so suppliers do not take a haircut.
            let with_deficit = AssetState {
                deficit_ray: U256::from(100) * RAY,
                drawn_shares: U256::ZERO,
                premium_shares: U256::ZERO,
                ..asset()
            };
            let without = AssetState {
                drawn_shares: U256::ZERO,
                premium_shares: U256::ZERO,
                ..asset()
            };

            assert_eq!(
                with_deficit.total_added_assets(RAY).unwrap()
                    - without.total_added_assets(RAY).unwrap(),
                U256::from(100)
            );
        }

        #[test]
        fn takes_the_protocol_cut_out_of_interest_accrued_since_the_checkpoint() {
            let state = AssetState {
                drawn_shares: U256::from(1_000_000),
                liquidity_fee: 1_000, // 10%
                premium_shares: U256::ZERO,
                premium_offset_ray: I256::ZERO,
                ..asset()
            };

            let at_checkpoint = state.total_added_assets(RAY).unwrap();
            let a_year_on = state
                .total_added_assets(state.drawn_index_at(T0 + YEAR).unwrap())
                .unwrap();

            // 50,000 accrues; the protocol takes 10%, suppliers get 45,000.
            assert_eq!(a_year_on - at_checkpoint, U256::from(45_000));
        }

        #[test]
        fn rounds_each_side_of_the_fee_difference_separately() {
            // The two forms agree unless both sides have a remainder and the
            // later one's is larger, which is what this constructs.
            let state = AssetState {
                liquidity: U256::ZERO,
                swept: U256::ZERO,
                realized_fees: U256::ZERO,
                drawn_shares: U256::from(1),
                premium_shares: U256::ZERO,
                premium_offset_ray: I256::ZERO,
                deficit_ray: U256::from(1),
                liquidity_fee: 10_000, // 100%, so the fee cannot round the gap away
                checkpoint_index: RAY,
                ..asset()
            };

            // owed(RAY) = RAY + 1 → 2;  owed(2·RAY+1) = 2·RAY + 2 → 3;  fee = 1.
            assert_eq!(
                state.total_added_assets(U256::from(2) * RAY + U256::from(1)),
                Ok(U256::from(3) - U256::from(1))
            );
        }

        #[test]
        fn charges_no_unrealized_fee_when_the_index_has_not_moved() {
            let state = AssetState {
                drawn_shares: U256::from(1_000_000),
                liquidity_fee: 1_000,
                ..asset()
            };

            assert_eq!(
                state.total_added_assets(RAY),
                AssetState {
                    liquidity_fee: 0,
                    ..state.clone()
                }
                .total_added_assets(RAY)
            );
        }
    }

    /// Where these quantities stop, measured rather than assumed.
    mod the_widths {
        use super::*;

        #[test]
        fn refuses_a_share_balance_that_would_leave_uint256() {
            // Both sides of the edge, which pins which term gives out.
            let at_the_edge = AssetState {
                drawn_shares: U256::MAX / RAY,
                checkpoint_index: RAY,
                ..asset()
            };
            let past_it = AssetState {
                drawn_shares: U256::MAX / RAY + U256::from(1),
                ..at_the_edge.clone()
            };

            assert!(at_the_edge.total_added_assets(RAY).is_ok());
            assert_eq!(past_it.total_added_assets(RAY), Err(Error::OutOfRange));
        }

        #[test]
        fn the_three_intermediates_are_the_widths_the_port_notes_record() {
            // Every row of the headroom table in `docs/rust-migration.md`.
            let uint120_max: U256 = (U256::from(1) << 120) - U256::from(1);
            assert_eq!(RAY.bit_len(), 90);

            // ray × ray, in `drawn_index_at`. 76 bits clear.
            assert_eq!(
                RAY.checked_mul(RAY + five_percent()).unwrap().bit_len(),
                180
            );

            // uint120 × ray, in `aggregated_owed_ray`. 46 clear.
            assert_eq!(uint120_max.checked_mul(RAY).unwrap().bit_len(), 210);

            // uint120 shares against a totalAssets of 2^128, in
            // `supplied_assets`. The notes' 248 counts `totalAssets` alone; the
            // multiply is against `totalAssets + VIRTUAL`, so the margin is 7.
            let total_assets = U256::from(1) << 128;
            assert_eq!(
                uint120_max.checked_mul(total_assets).unwrap().bit_len(),
                248
            );
            assert_eq!(
                uint120_max
                    .checked_mul(total_assets + VIRTUAL)
                    .unwrap()
                    .bit_len(),
                249
            );
        }

        #[test]
        fn the_widened_intermediate_carries_the_supply_side_past_that() {
            // Past the seven bits `shares * total` has no uint256 to live in,
            // but the quotient does — the argument for `Math.mulDiv`.
            let big = U256::from(1) << 200;
            let state = AssetState {
                liquidity: big,
                added_shares: big,
                drawn_shares: U256::ZERO,
                premium_shares: U256::ZERO,
                ..asset()
            };

            assert!(big.checked_mul(big + VIRTUAL).is_none());
            assert_eq!(state.supplied_assets(big, RAY), Ok(big));
        }
    }
}
