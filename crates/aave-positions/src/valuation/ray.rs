//! The protocol's fixed-point primitives, transcribed from `aave/aave-v4` at
//! commit `2524fe4`.
//!
//! **Rounding is not uniform and the directions are not interchangeable.**
//! Reconciliation is at zero tolerance (§9.2) — any nonzero drift is a bug, not
//! noise — so each of these reproduces one specific Solidity function and is
//! named after it rather than after what it does.
//!
//! All of them take non-negative inputs, because every caller is `uint256` on
//! chain. [`premium_ray`] is where a negative could arise, and it refuses for
//! the same reason the contract reverts.
//!
//! **Every intermediate is wider than the values it comes from.** A ray times
//! a ray is 180 bits and a share balance times an index is 210, so a product
//! goes through a 512-bit intermediate and narrows back with a check — the same
//! thing Solidity's `Math.mulDiv` achieves, and what the Uniswap V3 Rust ports
//! do. `#![deny(clippy::arithmetic_side_effects)]` on the crate is what keeps
//! that mechanical instead of a matter of discipline.

use alloy_primitives::{I256, U256, U512, uint};

use super::{Error, NegativePremium};

/// `MathUtils.RAY` — 1e27.
pub const RAY: U256 = uint!(1_000_000_000_000_000_000_000_000_000_U256);

/// `MathUtils.SECONDS_PER_YEAR` — 365 days, leap years ignored.
pub const SECONDS_PER_YEAR: U256 = uint!(31_536_000_U256);

/// `SharesMath.VIRTUAL_ASSETS` and `VIRTUAL_SHARES`, both 1e6.
pub const VIRTUAL: U256 = uint!(1_000_000_U256);

/// `PercentageMath.PERCENTAGE_FACTOR` — basis points.
pub const PERCENTAGE_FACTOR: U256 = uint!(10_000_U256);

/// What [`mul_div`] and [`mul_div_up`] name when their result leaves `uint256`.
///
/// One name for both, because a caller cannot predict which of the two steps
/// gives out first — `ray_mul_up(U256::MAX, RAY + 1)` fails at the narrowing,
/// not at the ceiling's `+ 1` — and it is the same quantity either way. Callers
/// that know what the product *means* name it themselves; these do not.
const PRODUCT: &str = "a * b / denominator";

/// Narrow a widened intermediate back to the `uint256` every quantity here is
/// on chain.
///
/// `alloy-primitives` re-exports ruint's `uint!` and `Uint` but not the crate
/// itself, so `UintTryTo` cannot be named from here — and taking a direct
/// `ruint` dependency to reach it is exactly the version-skew flaw refinery has
/// with `time`. The comparison is the check that trait would make.
pub(super) fn narrow(value: U512, what: &'static str) -> Result<U256, Error> {
    if value > U512::from(U256::MAX) {
        return Err(Error::OutOfRange { what });
    }
    Ok(value.wrapping_to::<U256>())
}

/// `a * b / denominator`, floored, over a 512-bit intermediate.
///
/// `Math.mulDiv(..., Rounding.Floor)`, and exported under that name as
/// [`mul_div_down`]; this is the shared body.
///
/// **Not `(a / denominator) * b`.** Integer division truncates the fractional
/// part, and in fixed-point the fractional part *is* the value: measured on a
/// real [`super::drawn_index_at`] — `a` = 1008055395294113139752655573, `b` ≈
/// [`RAY`] — that form comes out 0.7991% low, which would report near-zero
/// interest on every position.
fn mul_div(a: U256, b: U256, denominator: U256) -> Result<(U256, bool), Error> {
    if denominator.is_zero() {
        // No answer to give, and this is reachable: `mul_div_down` is public.
        return Err(Error::DivideByZero);
    }

    // The annotation is what picks `widening_mul`'s const generics, and it is
    // load-bearing rather than decorative: ruint asserts at run time that the
    // result width is exactly the sum of the operands'.
    let product: U512 = a.widening_mul(b);
    let (quotient, remainder) = product.div_rem(U512::from(denominator));

    Ok((narrow(quotient, PRODUCT)?, !remainder.is_zero()))
}

/// `Math.mulDiv(..., Rounding.Floor)`, as `SharesMath.toAssetsDown` uses it.
///
/// Headroom, measured on the one call that matters — `mul_div_down(shares,
/// totalAssets + VIRTUAL, addedShares + VIRTUAL)` in
/// [`super::supplied_assets`] — is **249 bits against 256, so seven to
/// spare**. That is the narrowest margin in the module, and the reason the
/// intermediate is widened rather than trusted. The tests pin all three
/// widths, so this comment cannot drift away from them.
pub fn mul_div_down(a: U256, b: U256, denominator: U256) -> Result<U256, Error> {
    Ok(mul_div(a, b, denominator)?.0)
}

/// The same, rounded up. `Math.mulDiv(..., Rounding.Ceil)`.
fn mul_div_up(a: U256, b: U256, denominator: U256) -> Result<U256, Error> {
    let (quotient, has_remainder) = mul_div(a, b, denominator)?;
    if !has_remainder {
        return Ok(quotient);
    }
    quotient
        .checked_add(U256::from(1))
        .ok_or(Error::OutOfRange { what: PRODUCT })
}

/// `WadRayMath.rayMulUp` — `ceil(a * b / RAY)`.
///
/// Headroom: the ray × ray product in [`super::drawn_index_at`] is 180 bits,
/// leaving 76.
pub fn ray_mul_up(a: U256, b: U256) -> Result<U256, Error> {
    mul_div_up(a, b, RAY)
}

/// `WadRayMath.fromRayUp` — `ceil(a / RAY)`.
///
/// Infallible, unlike its neighbours: the quotient is at most 2²⁵⁶ / 1e27 ≈
/// 2¹⁶⁶, so the ceiling's `+ 1` has ninety bits of room and there is nothing
/// for a caller to handle.
#[must_use]
pub fn from_ray_up(a: U256) -> U256 {
    let (quotient, remainder) = a.div_rem(RAY);
    if remainder.is_zero() {
        quotient
    } else {
        quotient.saturating_add(U256::from(1))
    }
}

/// `PercentageMath.percentMulDown` — `floor(value * bps / 10000)`.
pub fn percent_mul_down(value: U256, bps: U256) -> Result<U256, Error> {
    Ok(mul_div(value, bps, PERCENTAGE_FACTOR)?.0)
}

/// `MathUtils.calculateLinearInterest` — `RAY + rate * elapsed / SECONDS_PER_YEAR`.
///
/// Integer division, so the interest term floors before [`RAY`] is added.
///
/// **It takes the two instants, not an elapsed.** That is where the contract
/// does the subtraction too, and it reverts on the underflow —
/// [`Error::CheckpointAhead`] is that revert. A checkpoint ahead of the
/// valuation time means the caller mixed up two blocks, and every number that
/// follows would be quietly wrong.
pub fn linear_interest(rate: U256, checkpoint_at: u64, at: u64) -> Result<U256, Error> {
    let elapsed = at.checked_sub(checkpoint_at).ok_or_else(|| {
        Error::CheckpointAhead {
            // Safe by the branch: `checked_sub` only returned `None` because
            // `checkpoint_at` is the larger.
            seconds: checkpoint_at.saturating_sub(at),
        }
    })?;

    let interest = mul_div_down(rate, U256::from(elapsed), SECONDS_PER_YEAR)?;
    RAY.checked_add(interest).ok_or(Error::OutOfRange {
        what: "RAY + rate * elapsed / SECONDS_PER_YEAR",
    })
}

/// `Premium.calculatePremiumRay` — `premiumShares * drawnIndex - premiumOffsetRay`.
///
/// `premium_offset_ray` is `int200` on chain and genuinely negative, so the
/// subtraction can go either way. The contract closes it with `.toUint256()`,
/// which **reverts** on a negative — so a negative premium is not a state the
/// protocol can be in, and producing one here would mean the fold is wrong
/// rather than that the maths needs a signed branch. Refusing keeps that
/// distinction visible instead of returning a number nothing can interpret.
///
/// **The signature is signed and the body is not.** Branching on the sign and
/// adding the magnitude keeps every intermediate inside `U256`, where doing the
/// subtraction in `I256` would need a 257th bit for a product that already uses
/// 210 of them.
pub fn premium_ray(
    premium_shares: U256,
    premium_offset_ray: I256,
    drawn_index: U256,
) -> Result<U256, Error> {
    let product = premium_shares
        .checked_mul(drawn_index)
        .ok_or(Error::OutOfRange {
            what: "premiumShares * drawnIndex",
        })?;

    if premium_offset_ray.is_negative() {
        return product
            .checked_add(premium_offset_ray.unsigned_abs())
            .ok_or(Error::OutOfRange {
                what: "premiumShares * drawnIndex - premiumOffsetRay",
            });
    }

    let offset = premium_offset_ray.into_raw();
    product.checked_sub(offset).ok_or_else(|| {
        // Reached only when `offset` is the larger, so this subtraction is the
        // magnitude of a negative result rather than a wrap.
        Error::NegativePremium(Box::new(NegativePremium {
            shortfall: offset.wrapping_sub(product),
            shares: premium_shares,
            offset: premium_offset_ray,
            index: drawn_index,
        }))
    })
}

// Arithmetic here is the vector, not the code under test: reading
// `RAY + 1` beats reading `RAY.checked_add(U256::from(1)).unwrap()`, and ruint
// panics on overflow in every profile, so a vector that does not fit fails the
// test rather than shipping.
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
        // The one place the protocol rounds against the protocol. 9999 bps of 1
        // is zero, not one.
        assert_eq!(percent_mul_down(one(), U256::from(9_999)), Ok(U256::ZERO));
        assert_eq!(
            percent_mul_down(U256::from(10_000), U256::from(1_000)),
            Ok(U256::from(1_000))
        );
    }

    #[test]
    fn refuses_a_negative_premium_instead_of_inventing_one() {
        // `premium_offset_ray` is int200 and really goes negative, so the
        // subtraction can go either way — but the contract closes it with
        // `.toUint256()`, which reverts. A negative here means the fold is
        // wrong, not that the formula needs a signed branch.
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
        // Nothing in this crate can reach it — every denominator is a nonzero
        // constant or `added_shares + VIRTUAL` — but `mul_div_down` is public,
        // so the branch is a caller's to hit.
        assert_eq!(
            mul_div_down(one(), one(), U256::ZERO),
            Err(Error::DivideByZero)
        );
    }

    #[test]
    fn the_widened_intermediate_holds_a_product_that_uint256_cannot() {
        // `U256::MAX * 2` has no uint256 to live in, and this is what makes the
        // seven-bit margin on the supply side survivable: the product is exact
        // at 512 bits and only the quotient has to fit.
        assert!(U256::MAX.checked_mul(U256::from(2)).is_none());
        assert_eq!(
            mul_div_down(U256::MAX, U256::from(2), U256::from(2)),
            Ok(U256::MAX)
        );
    }

    #[test]
    fn refuses_a_quotient_that_would_leave_uint256() {
        // Exactly at the edge and one step past it, so the boundary is pinned
        // rather than assumed distant.
        assert_eq!(ray_mul_up(U256::MAX, RAY), Ok(U256::MAX));
        assert_eq!(
            ray_mul_up(U256::MAX, RAY + one()),
            Err(Error::OutOfRange { what: PRODUCT })
        );
    }
}
