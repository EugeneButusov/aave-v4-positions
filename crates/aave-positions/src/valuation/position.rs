//! What one position is worth, and the Hub asset state that decides it.
//!
//! `AssetLogic` and `SharesMath` as `Spoke.getUserDebt` and
//! `previewRemoveByShares` reach them, transcribed from `aave/aave-v4` at
//! commit `2524fe4`. Nothing here reads a database: the inputs are a Hub
//! asset's state and a user's shares, and Phase 2's store is what will fetch
//! them.

use alloy_primitives::{I256, U256, uint};

use super::Error;
use super::math::{
    from_ray_up, linear_interest, mul_div_down, percent_mul_down, premium_ray, ray_mul_up,
};

/// The Hub asset state one valuation reads.
///
/// A narrowed view of the `hub_assets_current` row — the fields the arithmetic
/// touches and nothing else, so a change to the row shape cannot silently alter
/// a formula.
///
/// **The widths are the column's.** The summed columns are
/// `Int256` on the way out of ClickHouse because they are sums of signed
/// deltas, and non-negative in aggregate; converting them is the store's job in
/// Phase 2, not this module's. `premium_offset_ray` is the exception and stays
/// signed, because it genuinely is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetState {
    pub liquidity: U256,
    pub added_shares: U256,
    pub drawn_shares: U256,
    pub swept: U256,
    pub premium_shares: U256,
    pub premium_offset_ray: I256,
    pub deficit_ray: U256,
    pub realized_fees: U256,
    /// Basis points, as `UpdateAssetConfig` carries it — `Nullable(UInt16)`.
    pub liquidity_fee: u16,
    /// The last `UpdateAsset`'s index — the checkpoint, not the current value.
    pub checkpoint_index: U256,
    /// `uint96` on chain, which is what makes `linear_interest` total.
    pub drawn_rate: u128,
    /// Unix seconds. `accrue()` sets it to the checkpoint block's timestamp.
    pub checkpoint_at: u64,
}

/// One user's shares in one reserve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionShares {
    pub supplied_shares: U256,
    pub drawn_shares: U256,
    pub premium_shares: U256,
    pub premium_offset_ray: I256,
}

/// What one position is worth, at [`PositionShares::value_at`]'s `at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Valuation {
    /// Underlying redeemable for `supplied_shares`, rounded down as the Hub does.
    pub supplied_amount: U256,
    pub drawn_debt: U256,
    pub premium_debt: U256,
    /// `drawn_debt + premium_debt`, which is what `getUserTotalDebt` returns.
    pub total_debt: U256,
    /// The index the valuation used — the checkpoint extrapolated to `at`.
    pub drawn_index: U256,
}

/// `SharesMath.VIRTUAL_ASSETS` and `VIRTUAL_SHARES`, both 1e6.
const VIRTUAL: U256 = uint!(1_000_000_U256);

impl AssetState {
    /// `AssetLogic.getDrawnIndex`, extrapolated to an arbitrary time.
    ///
    /// The index accrues every second and emits nothing (§5), so a balance is
    /// only meaningful with a time attached. What makes this exact rather than
    /// a model is §5.3: the Hub emits the *settled* index on every accrual, so
    /// this applies linear interest to an authoritative checkpoint instead of
    /// reconstructing a rate curve.
    ///
    /// **Two short-circuits, both from the contract and both load-bearing.**
    /// The index does not move when the checkpoint is already at `at`, nor when
    /// the asset owes nothing at all — and the second is on the *asset's*
    /// totals, not the user's, so a user with debt in an otherwise-idle asset
    /// still does not accrue. Skipping either inflates every debt in that
    /// asset.
    fn drawn_index_at(&self, at: u64) -> Result<U256, Error> {
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
    /// since the checkpoint, not yet folded into `realized_fees`.
    ///
    /// This is the one term §5.2 states in outline rather than in full, so it is
    /// transcribed rather than reconstructed. Note it is a difference of two
    /// *separately rounded* values — `from_ray_up(after) - from_ray_up(before)`
    /// — and not `from_ray_up(after - before)`, which would differ by a wei
    /// whenever both have a remainder.
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
    /// Suppliers are paid out of accrued debt, so this depends on `drawn_index`
    /// and therefore on time: the supply side is a per-second quantity too, not
    /// just the debt side (§5.2).
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
    /// **Not an index.** ERC-4626 virtual assets and shares of `1e6` each pad
    /// the ratio against manipulation, and the padding is inside the division
    /// rather than applied after it, so it cannot be factored out. Rounds
    /// **down**, where the debt side rounds up.
    fn supplied_assets(&self, shares: U256, drawn_index: U256) -> Result<U256, Error> {
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

impl PositionShares {
    /// One position's balances at `at`, in token units.
    ///
    /// `at` is Unix seconds and is the caller's choice, exactly as
    /// `block.timestamp` is the chain's: `getUserDebt` at `latest` is stored
    /// shares times an index extrapolated to the head block, and this is the
    /// same computation with the time named rather than implied.
    ///
    /// The debt side rounds **up** throughout — `rayMulUp` on the drawn part
    /// and `fromRayUp` on the premium, each separately, which is what
    /// `Spoke.getUserDebt` does before summing. Rounding the total once instead
    /// would be a wei light.
    pub fn value_at(&self, asset: &AssetState, at: u64) -> Result<Valuation, Error> {
        let drawn_index = asset.drawn_index_at(at)?;

        let drawn_debt = ray_mul_up(self.drawn_shares, drawn_index)?;
        let premium_debt = from_ray_up(premium_ray(
            self.premium_shares,
            self.premium_offset_ray,
            drawn_index,
        )?);

        Ok(Valuation {
            supplied_amount: asset.supplied_assets(self.supplied_shares, drawn_index)?,
            drawn_debt,
            premium_debt,
            total_debt: drawn_debt
                .checked_add(premium_debt)
                .ok_or(Error::OutOfRange)?,
            drawn_index,
        })
    }
}

// Same reason as `math`'s: the arithmetic in a vector is the vector, and
// ruint panics on overflow in every profile, so a fixture that does not fit
// fails the test.
#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use super::*;
    use crate::valuation::RAY;

    const HOUR: u64 = 3_600;
    const YEAR: u64 = 365 * 24 * HOUR;
    const T0: u64 = 1_785_000_000;

    /// 5% per annum, RAY-scaled, as `drawn_rate` arrives on `UpdateAsset`.
    const FIVE_PERCENT: u128 = 10u128.pow(27) / 20;

    fn five_percent() -> U256 {
        U256::from(FIVE_PERCENT)
    }

    fn asset() -> AssetState {
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

    fn position() -> PositionShares {
        PositionShares {
            supplied_shares: U256::ZERO,
            drawn_shares: U256::ZERO,
            premium_shares: U256::ZERO,
            premium_offset_ray: I256::ZERO,
        }
    }

    mod the_drawn_index {
        use super::*;

        #[test]
        fn applies_linear_interest_from_the_checkpoint() {
            // 5% for exactly a year, on an index of 1.0.
            assert_eq!(asset().drawn_index_at(T0 + YEAR), Ok(RAY + five_percent()));
        }

        #[test]
        fn compounds_only_where_a_checkpoint_landed() {
            // Two years in one step is 10%, not 10.25% — interest is linear
            // between checkpoints and compounds only when one lands (§5.1).
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
            // The short-circuit is on the *asset's* totals. Dropping it makes
            // every idle asset's index creep upward and inflates any debt later
            // drawn.
            let idle = AssetState {
                drawn_shares: U256::ZERO,
                premium_shares: U256::ZERO,
                ..asset()
            };

            assert_eq!(idle.drawn_index_at(T0 + YEAR), Ok(RAY));
        }

        #[test]
        fn still_accrues_when_only_the_premium_is_outstanding() {
            // `drawn_shares == 0 && premium_shares == 0` — both, not either.
            let premium_only = AssetState {
                drawn_shares: U256::ZERO,
                premium_shares: U256::from(1),
                ..asset()
            };

            assert!(premium_only.drawn_index_at(T0 + YEAR).unwrap() > RAY);
        }

        #[test]
        fn the_checkpoint_short_circuit_is_load_bearing_at_the_top_of_the_range() {
            // Below `rayMulUp`'s guard the branch is invisible: the else-branch
            // at `at == checkpoint_at` is `ray_mul_up(index, RAY)`, which is
            // `index`. That is why deleting it changes nothing in a language
            // with arbitrary precision, and the design notes record the
            // TypeScript suite surviving exactly that deletion.
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

            // Past the guard it is the difference between an answer and none,
            // because `index * RAY` is where the contract reverts.
            let past_the_guard = AssetState {
                checkpoint_index: U256::MAX,
                ..asset()
            };

            assert_eq!(past_the_guard.drawn_index_at(T0), Ok(U256::MAX));
            assert!(ray_mul_up(U256::MAX, RAY).is_err());
        }

        #[test]
        fn refuses_a_checkpoint_in_the_future() {
            // On chain this reverts. Here it means two different blocks were
            // mixed up, and every number downstream would be quietly wrong.
            assert_eq!(
                asset().drawn_index_at(T0 - 1),
                Err(Error::CheckpointAhead { seconds: 1 })
            );
        }
    }

    mod debt {
        use super::*;

        #[test]
        fn rounds_each_component_up_separately_as_the_spoke_does() {
            // getUserDebt returns (rayMulUp(shares, index), fromRayUp(premiumRay))
            // and getUserTotalDebt sums those two already-rounded numbers.
            // Rounding the sum once instead is a wei light.
            let state = AssetState {
                checkpoint_index: RAY + U256::from(1),
                premium_shares: U256::from(1),
                ..asset()
            };
            let held = PositionShares {
                drawn_shares: U256::from(1),
                premium_shares: U256::from(1),
                ..position()
            };

            let valued = held.value_at(&state, T0).unwrap();

            assert_eq!(valued.drawn_debt, U256::from(2));
            assert_eq!(valued.premium_debt, U256::from(2));
            assert_eq!(valued.total_debt, U256::from(4));
        }

        #[test]
        fn grows_with_time_on_a_fixed_share_balance() {
            let held = PositionShares {
                drawn_shares: U256::from(1_000_000),
                ..position()
            };

            let now = held.value_at(&asset(), T0).unwrap().total_debt;
            let later = held.value_at(&asset(), T0 + YEAR).unwrap().total_debt;

            // The whole reason a share balance is not a balance (§5).
            assert_eq!(now, U256::from(1_000_000));
            assert_eq!(later, U256::from(1_050_000));
        }

        #[test]
        fn is_zero_for_a_position_with_no_debt() {
            assert_eq!(
                position().value_at(&asset(), T0 + YEAR).unwrap().total_debt,
                U256::ZERO
            );
        }
    }

    mod supply {
        use super::*;

        #[test]
        fn pads_the_ratio_with_virtual_assets_and_shares() {
            // 1e6 each, inside the division rather than applied after — the
            // padding cannot be factored out, and dropping it changes every
            // result.
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

            // liquidity + swept + owed − realizedFees = 1000 + 500 + 2000 − 300.
            assert_eq!(state.total_added_assets(RAY), Ok(U256::from(3_200)));
        }

        #[test]
        fn carries_bad_debt_as_if_still_owed() {
            // §12.3: a deficit stays inside aggregatedOwedRay until eliminated,
            // so suppliers hold shares partly backed by it rather than taking a
            // haircut.
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

            // 50,000 of interest accrues; the protocol takes 10% and suppliers
            // get the other 45,000.
            assert_eq!(a_year_on - at_checkpoint, U256::from(45_000));
        }

        #[test]
        fn rounds_each_side_of_the_fee_difference_separately() {
            // `from_ray_up(after) - from_ray_up(before)`, not
            // `from_ray_up(after - before)`. The two agree unless both sides
            // have a remainder and the later one's is larger, which is what
            // this constructs: owed goes from RAY+1 to 2·RAY+2, so the
            // separately-rounded difference is 3−2 = 1 while rounding the
            // difference once gives ceil((RAY+1)/RAY) = 2.
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

            // Both short-circuits in getUnrealizedFees: same index, and zero fee.
            assert_eq!(
                state.total_added_assets(RAY),
                AssetState {
                    liquidity_fee: 0,
                    ..state.clone()
                }
                .total_added_assets(RAY)
            );
        }

        #[test]
        fn rises_for_a_supplier_as_debt_accrues_underneath_them() {
            let state = AssetState {
                drawn_shares: U256::from(1_000_000),
                added_shares: U256::from(1_000_000),
                ..asset()
            };
            let held = PositionShares {
                supplied_shares: U256::from(1_000_000),
                ..position()
            };

            let now = held.value_at(&state, T0).unwrap().supplied_amount;
            let later = held.value_at(&state, T0 + YEAR).unwrap().supplied_amount;

            // The supply side is a per-second quantity too, because it is paid
            // out of the debt side (§5.2).
            assert!(later > now);
        }
    }

    mod values_past_2_53 {
        use super::*;

        #[test]
        fn stays_exact_on_a_realistic_share_balance() {
            // A real Repay in the Spoke fixture carries this many shares, and
            // float64 would round the tail (§7.5).
            let shares = uint!(422166581625087607993_U256);
            let held = PositionShares {
                drawn_shares: shares,
                ..position()
            };
            let state = AssetState {
                checkpoint_index: RAY,
                drawn_shares: shares,
                ..asset()
            };

            assert_eq!(held.value_at(&state, T0).unwrap().drawn_debt, shares);
        }
    }

    /// Where these quantities stop, measured rather than assumed.
    mod the_widths {
        use super::*;

        #[test]
        fn values_a_uint120_share_balance_against_a_ray_index() {
            // `drawn_shares * drawn_index` is the 210-bit intermediate, 46 bits
            // clear of uint256. This is the realistic worst case, and it is
            // exact rather than merely fitting.
            let shares = (U256::from(1) << 120) - U256::from(1);
            let state = AssetState {
                drawn_shares: shares,
                checkpoint_index: RAY,
                ..asset()
            };
            let held = PositionShares {
                drawn_shares: shares,
                ..position()
            };

            assert_eq!(held.value_at(&state, T0).unwrap().drawn_debt, shares);
        }

        #[test]
        fn refuses_a_share_balance_that_would_leave_uint256() {
            // Both sides of the edge, which pins the term that gives out more
            // firmly than a label on the error would: at `MAX / RAY` the
            // `drawnShares * drawnIndex` product is the largest that fits, and
            // one share more needs a 257th bit.
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
            // Prose in a doc comment drifts; this does not. Every row of the
            // headroom table in `docs/rust-migration.md`, measured.
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
            // `supplied_assets` — the narrowest margin in the module.
            //
            // **The Port notes' 248 counts `totalAssets` alone.** The multiply
            // is against `totalAssets + VIRTUAL`, because the ERC-4626 padding
            // is inside the division rather than applied after it, so the
            // intermediate is one bit wider and the margin is seven.
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
            // Past the seven bits, `shares * total` has no uint256 to live in
            // — but the quotient does, and that is the whole argument for
            // `Math.mulDiv` over `(a / c) * b`.
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
