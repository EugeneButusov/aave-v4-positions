//! One position's balances, in token units, at an instant the caller names.
//!
//! Four modules, and the split follows what each answers to. `position` is the
//! fold's arithmetic and `price` is `SpokeUtils.toValue`; they share no input,
//! which is why they are not one file. `math` is the contracts' libraries
//! underneath both, and `error` is what all three refuse with.
//!
//! This file holds the wiring and the one constant both halves are scaled in.

mod error;
mod math;
mod position;
mod price;

use alloy_primitives::{U256, uint};

pub use error::{Error, NegativePremium};
pub use position::{AssetState, PositionShares, Valuation, value_position};
pub use price::{USD, to_value};

/// `1e27` — the unit every index and rate here is scaled in.
///
/// Not in `math` beside the functions that divide by it, because it is not
/// theirs: the chain declares it twice, once in `WadRayMath` and once in
/// `MathUtils`. It is the protocol's unit, and here it is the unit of
/// [`AssetState::checkpoint_index`] and [`AssetState::drawn_rate`].
const RAY: U256 = uint!(1_000_000_000_000_000_000_000_000_000_U256);
