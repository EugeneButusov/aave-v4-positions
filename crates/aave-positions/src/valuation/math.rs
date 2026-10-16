//! The contracts' math libraries, transcribed from `aave/aave-v4` at commit
//! `2524fe4` — `WadRayMath`, `PercentageMath`, `MathUtils`, `Premium`, and
//! OpenZeppelin's `Math` as `SharesMath` reaches it.
//!
//! Each function reproduces one Solidity function and is named after it,
//! because reconciliation is at zero tolerance (§9.2) and the rounding
//! directions are not interchangeable. Each also overflows where its own
//! contract overflows, and the three contracts disagree — see "Arithmetic
//! widths" in `docs/design-notes.md`.

use alloy_primitives::{I256, U256, U512, uint};

use super::{Error, NegativePremium, RAY};

/// Narrow a widened intermediate back to `uint256`.
pub(super) fn narrow(value: U512) -> Result<U256, Error> {
    if value > U512::from(U256::MAX) {
        return Err(Error::OutOfRange);
    }
    Ok(value.wrapping_to::<U256>())
}

/// `Math.mulDiv(..., Rounding.Floor)`, as `SharesMath.toAssetsDown` uses it.
///
/// The 512-bit intermediate is the transcription: `mulDiv` panics only when the
/// quotient leaves `uint256`, so `uint256` here would refuse inputs the chain
/// accepts. Not `(a / denominator) * b` — that form measured 0.7991% low.
pub(super) fn mul_div_down(a: U256, b: U256, denominator: U256) -> Result<U256, Error> {
    if denominator.is_zero() {
        return Err(Error::DivideByZero);
    }

    // The annotation picks `widening_mul`'s const generics; ruint asserts at
    // run time that the result width is the sum of the operands'.
    let product: U512 = a.widening_mul(b);

    narrow(product.wrapping_div(U512::from(denominator)))
}

/// `WadRayMath.rayMulUp` — `ceil(a * b / RAY)`.
///
/// Checked in `uint256`, because the contract's guard fires on the product even
/// where the result would have fit.
pub(super) fn ray_mul_up(a: U256, b: U256) -> Result<U256, Error> {
    let product = a.checked_mul(b).ok_or(Error::OutOfRange)?;

    let (quotient, remainder) = product.div_rem(RAY);
    if remainder.is_zero() {
        return Ok(quotient);
    }
    Ok(quotient.saturating_add(U256::from(1)))
}

/// `WadRayMath.fromRayUp` — `ceil(a / RAY)`.
///
/// Infallible: the quotient is at most 2²⁵⁶ / 1e27, so the `+ 1` has room.
#[must_use]
pub(super) fn from_ray_up(a: U256) -> U256 {
    let (quotient, remainder) = a.div_rem(RAY);
    if remainder.is_zero() {
        quotient
    } else {
        quotient.saturating_add(U256::from(1))
    }
}

/// `PercentageMath.percentMulDown` — `floor(value * bps / 10000)`.
pub(super) fn percent_mul_down(value: U256, bps: U256) -> Result<U256, Error> {
    /// `PercentageMath.PERCENTAGE_FACTOR` — basis points.
    const PERCENTAGE_FACTOR: U256 = uint!(10_000_U256);

    let product = value.checked_mul(bps).ok_or(Error::OutOfRange)?;

    Ok(product.wrapping_div(PERCENTAGE_FACTOR))
}

/// `MathUtils.calculateLinearInterest` — `RAY + rate * elapsed / SECONDS_PER_YEAR`.
///
/// Takes the two instants rather than an elapsed, because that is where the
/// contract subtracts and reverts: [`Error::CheckpointAhead`] is that revert.
pub(super) fn linear_interest(rate: u128, checkpoint_at: u64, at: u64) -> Result<U256, Error> {
    /// `MathUtils.SECONDS_PER_YEAR` — 365 days, leap years ignored.
    const SECONDS_PER_YEAR: U256 = uint!(31_536_000_U256);

    let elapsed = at
        .checked_sub(checkpoint_at)
        .ok_or(Error::CheckpointAhead {
            seconds: checkpoint_at.saturating_sub(at),
        })?;

    // Saturating because 192 bits cannot reach 256 and the divisor is a nonzero
    // constant; the crate's lint will not take an unadorned operator.
    let interest = U256::from(rate)
        .saturating_mul(U256::from(elapsed))
        .wrapping_div(SECONDS_PER_YEAR);

    Ok(RAY.saturating_add(interest))
}

/// `Premium.calculatePremiumRay` — `premiumShares * drawnIndex - premiumOffsetRay`.
///
/// The contract closes with `.toUint256()`, which reverts on a negative, so a
/// negative premium means the fold is wrong rather than that this needs a
/// signed branch. Signed signature, unsigned body: branching on the sign keeps
/// every intermediate inside `U256`.
pub(super) fn premium_ray(
    premium_shares: U256,
    premium_offset_ray: I256,
    drawn_index: U256,
) -> Result<U256, Error> {
    let product = premium_shares
        .checked_mul(drawn_index)
        .ok_or(Error::OutOfRange)?;

    if premium_offset_ray.is_negative() {
        return product
            .checked_add(premium_offset_ray.unsigned_abs())
            .ok_or(Error::OutOfRange);
    }

    let offset = premium_offset_ray.into_raw();
    product.checked_sub(offset).ok_or_else(|| {
        Error::NegativePremium(Box::new(NegativePremium {
            shortfall: offset.wrapping_sub(product),
            shares: premium_shares,
            offset: premium_offset_ray,
            index: drawn_index,
        }))
    })
}

// Arithmetic here is the vector, not the code under test, and ruint panics on
// overflow in every profile — a vector that does not fit fails the test.
#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use super::*;

    fn one() -> U256 {
        U256::from(1)
    }

    #[test]
    fn rounds_a_ray_product_up_and_only_when_there_is_a_remainder() {
        assert_eq!(ray_mul_up(U256::from(2), RAY), Ok(U256::from(2)));
        assert_eq!(ray_mul_up(one(), RAY + one()), Ok(U256::from(2)));
        assert_eq!(ray_mul_up(U256::ZERO, RAY), Ok(U256::ZERO));
    }

    #[test]
    fn rounds_a_ray_division_up_and_only_when_there_is_a_remainder() {
        assert_eq!(from_ray_up(RAY), one());
        assert_eq!(from_ray_up(RAY + one()), U256::from(2));
        assert_eq!(from_ray_up(one()), one());
        assert_eq!(from_ray_up(U256::ZERO), U256::ZERO);
    }

    #[test]
    fn rounds_a_percentage_down() {
        // The one place the protocol rounds against the protocol.
        assert_eq!(percent_mul_down(one(), U256::from(9_999)), Ok(U256::ZERO));
        assert_eq!(
            percent_mul_down(U256::from(10_000), U256::from(1_000)),
            Ok(U256::from(1_000))
        );
    }

    #[test]
    fn refuses_a_negative_premium_instead_of_inventing_one() {
        let refused = premium_ray(one(), I256::try_from(RAY * U256::from(2)).unwrap(), RAY);
        assert!(matches!(refused, Err(Error::NegativePremium(_))));
        assert_eq!(
            refused.unwrap_err().to_string(),
            format!(
                "premium is negative (-{RAY}): shares 1, offset {}, index {RAY}",
                RAY * U256::from(2)
            )
        );

        assert_eq!(premium_ray(one(), I256::MINUS_ONE, RAY), Ok(RAY + one()));
    }

    #[test]
    fn refuses_a_zero_denominator() {
        // Unreachable from this crate, but `mul_div_down` is a caller's to hit.
        assert_eq!(
            mul_div_down(one(), one(), U256::ZERO),
            Err(Error::DivideByZero)
        );
    }

    #[test]
    fn the_widened_intermediate_holds_a_product_that_uint256_cannot() {
        assert!(U256::MAX.checked_mul(U256::from(2)).is_none());
        assert_eq!(
            mul_div_down(U256::MAX, U256::from(2), U256::from(2)),
            Ok(U256::MAX)
        );
    }

    #[test]
    fn refuses_a_product_where_the_contract_reverts() {
        // The guard fires on the product: `MAX * RAY / RAY` would fit, but the
        // chain never gets there.
        assert_eq!(ray_mul_up(U256::MAX, RAY), Err(Error::OutOfRange));
        assert_eq!(ray_mul_up(U256::MAX / RAY, RAY), Ok(U256::MAX / RAY));
    }

    #[test]
    fn floors_the_interest_term_before_adding_ray() {
        // One second at a rate of 1 is worth nothing; it takes a rate of
        // SECONDS_PER_YEAR for that second to be worth a single wei.
        assert_eq!(linear_interest(1, 0, 1), Ok(RAY));
        assert_eq!(linear_interest(31_536_000, 0, 1), Ok(RAY + one()));
    }

    #[test]
    fn applies_the_rate_linearly_and_does_not_compound() {
        const FIVE_PERCENT: u128 = 10u128.pow(27) / 20;
        const YEAR: u64 = 31_536_000;

        assert_eq!(
            linear_interest(FIVE_PERCENT, 0, YEAR),
            Ok(RAY + U256::from(FIVE_PERCENT))
        );
        assert_eq!(
            linear_interest(FIVE_PERCENT, 0, 2 * YEAR),
            Ok(RAY + U256::from(2 * FIVE_PERCENT))
        );
        assert_eq!(linear_interest(FIVE_PERCENT, 0, 0), Ok(RAY));
    }

    #[test]
    fn refuses_a_checkpoint_ahead_and_says_how_far() {
        // Two distances, so the test cannot pass against a constant.
        assert_eq!(
            linear_interest(1, 90, 0),
            Err(Error::CheckpointAhead { seconds: 90 })
        );
        assert_eq!(
            linear_interest(1, 1, 0),
            Err(Error::CheckpointAhead { seconds: 1 })
        );
    }

    #[test]
    fn linear_interest_cannot_overflow_at_the_far_end_of_its_types() {
        // `uint96 × uint40` on chain. Even `u128` against `u64` stays inside
        // `uint256`, which is why the contract's assembly has no guard.
        assert_eq!(
            U256::from(u128::MAX)
                .checked_mul(U256::from(u64::MAX))
                .map(|p| p.bit_len()),
            Some(192)
        );
        assert!(linear_interest(u128::MAX, 0, u64::MAX).is_ok());
    }
}
