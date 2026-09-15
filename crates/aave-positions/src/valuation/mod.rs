//! One position's balances, in token units, at an instant the caller names.
//!
//! The split follows what each part answers to. `asset` is the Hub asset and
//! the five formulas that hang off it; `position` is a user's shares and the
//! one call that values them against an asset; `price` is `SpokeUtils.toValue`,
//! which shares no input with either. `math` is the contracts' libraries
//! underneath all three, `error` is what they refuse with, and `fixtures` is
//! the vectors `asset` and `position` both build on.
//!
//! This file holds the wiring and the one value they are all scaled in.

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scale::RAY_DECIMALS;

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
}
