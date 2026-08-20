//! One user's shares, and what they are worth at an instant.
//!
//! `Spoke.getUserDebt` and `previewRemoveByShares`, transcribed from
//! `aave/aave-v4` at commit `2524fe4`. Nothing here reads a database: the
//! inputs are a Hub asset's state and a user's shares, and Phase 2's store is
//! what will fetch them.

use alloy_primitives::{I256, U256};

use super::Error;
use super::asset::AssetState;
use super::math::{from_ray_up, premium_ray, ray_mul_up};

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
    use alloy_primitives::uint;

    use super::*;
    use crate::valuation::RAY;
    use crate::valuation::fixtures::*;

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

    /// Past 2^53, and at the widths the port notes record.
    mod exactness {
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
    }
}
