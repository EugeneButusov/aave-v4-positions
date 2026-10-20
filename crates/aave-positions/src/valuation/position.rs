//! One user's shares, and what they are worth at an instant.
//!
//! `Spoke.getUserDebt` and `previewRemoveByShares`, transcribed from
//! `aave/aave-v4` at commit `2524fe4`. Nothing here reads a database.

use alloy_primitives::{I256, U256};

use super::Error;
use super::asset::AssetState;
use super::math::{from_ray_up, premium_ray, ray_mul_up};

/// One user's shares in one reserve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PositionShares {
    pub(crate) supplied_shares: U256,
    pub(crate) drawn_shares: U256,
    pub(crate) premium_shares: U256,
    pub(crate) premium_offset_ray: I256,
}

/// What one position is worth, at `PositionShares::value_at`'s `at`.
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
    /// `at` is the caller's choice as `block.timestamp` is the chain's:
    /// `getUserDebt` at `latest` is the same computation with the time implied.
    ///
    /// The debt side rounds up throughout, and each part separately, as
    /// `Spoke.getUserDebt` does before summing — rounding the total once
    /// instead would be a wei light.
    pub(crate) fn value_at(&self, asset: &AssetState, at: u64) -> Result<Valuation, Error> {
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

// As in `math`: the arithmetic in a vector is the vector.
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
            // `getUserTotalDebt` sums two already-rounded numbers; rounding
            // the sum once instead is a wei light.
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

            // The supply side is per-second too, being paid out of the debt
            // side (§5.2).
            assert!(later > now);
        }
    }

    /// Past 2^53, and at the widths the port notes record.
    mod exactness {
        use super::*;

        #[test]
        fn stays_exact_on_a_realistic_share_balance() {
            // A real Repay carries this many shares; float64 would round the
            // tail (§7.5).
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
            // The 210-bit intermediate, 46 bits clear of uint256.
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
