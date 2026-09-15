//! One position's balances, in token units, at an instant the caller names.
//!
//! The split follows what each part answers to. `asset` is the Hub asset and
//! the five formulas that hang off it; `position` is a user's shares and the
//! one call that values them against an asset; `price` is `SpokeUtils.toValue`,
//! which shares no input with either. `math` is the contracts' libraries
//! underneath all three, `error` is what they refuse with, and `fixtures` is
//! the vectors `asset` and `position` both build on.
//!
//! This file holds the wiring and the units they are all scaled in.

mod asset;
mod error;
#[cfg(test)]
mod fixtures;
mod math;
mod position;
mod price;

use alloy_primitives::{U256, uint};

pub use error::{Error, NegativePremium};
pub use position::Valuation;
pub use price::to_value;

pub(crate) use asset::AssetState;
pub(crate) use position::PositionShares;

/// `1e27` — the unit every index and rate here is scaled in.
///
/// Not in `math` beside the functions that divide by it, because it is not
/// theirs: the chain declares it twice, once in `WadRayMath` and once in
/// `MathUtils`. It is the protocol's unit, and here it is the unit of
/// [`AssetState::checkpoint_index`] and [`AssetState::drawn_rate`].
const RAY: U256 = uint!(1_000_000_000_000_000_000_000_000_000_U256);

/// How many of a ray's digits are fractional: the protocol's unit as an
/// exponent, which is the form a caller rendering one needs.
/// `ray_is_ten_to_its_decimals` holds it to the value this module divides by.
pub const RAY_DECIMALS: u8 = 27;

/// `WadRayMath.WAD_DECIMALS`, which [`to_value`] normalises an amount to.
pub const WAD_DECIMALS: u8 = 18;

/// `SpokeUtils.ORACLE_DECIMALS`, and what `Spoke`'s constructor requires of an
/// oracle.
pub const ORACLE_DECIMALS: u8 = 8;

/// How many digits of a [`to_value`] result are fractional.
///
/// Derived rather than written: it is an amount at [`WAD_DECIMALS`] times a
/// price at [`ORACLE_DECIMALS`], which is what makes `1e26` one dollar (§7.1).
pub const VALUE_DECIMALS: u8 = WAD_DECIMALS + ORACLE_DECIMALS;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_is_ten_to_its_decimals() {
        // The two spellings of one protocol constant, held to each other: a
        // caller scaling by the exponent and this module dividing by the value
        // must mean the same thing.
        assert_eq!(
            U256::from(10).pow(U256::from(RAY_DECIMALS)),
            RAY,
            "RAY_DECIMALS and RAY disagree"
        );
    }

    #[test]
    fn a_value_is_an_amount_times_a_price() {
        assert_eq!(VALUE_DECIMALS, 26);
    }
}
