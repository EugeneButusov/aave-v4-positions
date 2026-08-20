//! The contracts' math libraries, transcribed from `aave/aave-v4` at commit
//! `2524fe4` — `WadRayMath`, `PercentageMath`, `MathUtils`, `Premium`, and
//! OpenZeppelin's `Math` as `SharesMath` reaches it.
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
//! **Each one overflows where its own contract overflows, and the three do not
//! agree.** `WadRayMath.rayMulUp` and `PercentageMath.percentMulDown` multiply
//! in `uint256` and revert when the product does not fit, guard first —
//! `if iszero(or(iszero(b), iszero(gt(a, div(not(0), b))))) { revert }`.
//! `SharesMath.toAssetsDown` goes through OpenZeppelin's `Math.mulDiv`, which
//! takes the full 512-bit product and only gives out when the *quotient* would
//! not fit. So [`mul_div_down`] is the one that widens, and it widens because
//! that is the transcription: doing it in `uint256` would refuse inputs the
//! chain accepts, and widening the other two would accept inputs the chain
//! refuses.
//!
//! `#![deny(clippy::arithmetic_side_effects)]` on the crate is what keeps that
//! deliberate instead of a matter of discipline.

use alloy_primitives::{I256, U256, U512, uint};

use super::{Error, NegativePremium, RAY};

/// Narrow a widened intermediate back to the `uint256` every quantity here is
/// on chain.
///
/// `alloy-primitives` re-exports ruint's `uint!` and `Uint` but not the crate
/// itself, so `UintTryTo` cannot be named from here — and taking a direct
/// `ruint` dependency to reach it is exactly the version-skew flaw refinery has
/// with `time`. The comparison is the check that trait would make.
pub(super) fn narrow(value: U512) -> Result<U256, Error> {
    if value > U512::from(U256::MAX) {
        return Err(Error::OutOfRange);
    }
    Ok(value.wrapping_to::<U256>())
}

/// `Math.mulDiv(..., Rounding.Floor)`, as `SharesMath.toAssetsDown` uses it.
///
/// **The 512-bit intermediate is the transcription, not a precaution.**
/// OpenZeppelin's `mulDiv` takes the full product through `mul512` and panics
/// only when the quotient would leave `uint256`, so computing this in `uint256`
/// would refuse inputs the chain accepts — a supply side whose
/// `shares * totalAssets` passes 2²⁵⁶ while the amount it redeems for does not.
///
/// **Not `(a / denominator) * b`.** Integer division truncates the fractional
/// part, and in fixed-point the fractional part *is* the value: measured on a
/// real [`super::drawn_index_at`] — `a` = 1008055395294113139752655573, `b` ≈
/// [`RAY`] — that form comes out 0.7991% low, which would report near-zero
/// interest on every position.
///
/// Headroom on the one call that matters — `mul_div_down(shares,
/// totalAssets + VIRTUAL, addedShares + VIRTUAL)` in
/// [`super::supplied_assets`] — is 249 bits against 256, seven to spare. The
/// tests pin that and the other two widths, so this cannot drift.
pub(super) fn mul_div_down(a: U256, b: U256, denominator: U256) -> Result<U256, Error> {
    if denominator.is_zero() {
        // `mulDiv` panics with `DIVISION_BY_ZERO` here. No answer to give, and
        // this is public, so a caller can ask.
        return Err(Error::DivideByZero);
    }

    // The annotation is what picks `widening_mul`'s const generics, and it is
    // load-bearing rather than decorative: ruint asserts at run time that the
    // result width is exactly the sum of the operands'.
    let product: U512 = a.widening_mul(b);

    narrow(product.wrapping_div(U512::from(denominator)))
}

/// `WadRayMath.rayMulUp` — `ceil(a * b / RAY)`.
///
/// **In `uint256`, with the product checked before the divide, because that is
/// what the contract does.** Its guard is `a <= type(uint256).max / b`, and it
/// reverts there even when `a * b / RAY` would have fit — so widening would
/// answer where the chain refuses to. [`Error::OutOfRange`] lands on exactly
/// the inputs that revert.
///
/// Headroom: the ray × ray product in [`super::drawn_index_at`] is 180 bits,
/// leaving 76.
pub(super) fn ray_mul_up(a: U256, b: U256) -> Result<U256, Error> {
    let product = a.checked_mul(b).ok_or(Error::OutOfRange)?;

    let (quotient, remainder) = product.div_rem(RAY);
    if remainder.is_zero() {
        return Ok(quotient);
    }
    // The quotient is at most 2²⁵⁶ / 1e27, so the ceiling has room to spare.
    Ok(quotient.saturating_add(U256::from(1)))
}

/// `WadRayMath.fromRayUp` — `ceil(a / RAY)`.
///
/// Infallible, unlike its neighbours: the quotient is at most 2²⁵⁶ / 1e27 ≈
/// 2¹⁶⁶, so the ceiling's `+ 1` has ninety bits of room and there is nothing
/// for a caller to handle.
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
///
/// `uint256` and a guard, the same shape as [`ray_mul_up`] and for the same
/// reason: the contract reverts on the product, not on the result.
pub(super) fn percent_mul_down(value: U256, bps: U256) -> Result<U256, Error> {
    /// `PercentageMath.PERCENTAGE_FACTOR` — basis points.
    const PERCENTAGE_FACTOR: U256 = uint!(10_000_U256);

    let product = value.checked_mul(bps).ok_or(Error::OutOfRange)?;

    Ok(product.wrapping_div(PERCENTAGE_FACTOR))
}

/// `MathUtils.calculateLinearInterest` — `RAY + rate * elapsed / SECONDS_PER_YEAR`.
///
/// Integer division, so the interest term floors before [`RAY`] is added.
///
/// **Nothing here can overflow, and the contract says so with its types.** It
/// takes `(uint96 rate, uint40 lastUpdateTimestamp)`, which is why its assembly
/// carries no overflow guard where every neighbour in `WadRayMath` and
/// `PercentageMath` does: 96 bits times 40 cannot fill 256. `u128` is the
/// narrowest Rust integer that holds a `uint96`, and even at its own maximum
/// against a full `u64` the product is 192 bits.
///
/// **It takes the two instants, not an elapsed.** That is where the contract
/// does the subtraction too, and it reverts on the underflow —
/// [`Error::CheckpointAhead`] is that revert. A checkpoint ahead of the
/// valuation time means the caller mixed up two blocks, and every number that
/// follows would be quietly wrong.
pub(super) fn linear_interest(rate: u128, checkpoint_at: u64, at: u64) -> Result<U256, Error> {
    /// `MathUtils.SECONDS_PER_YEAR` — 365 days, leap years ignored.
    const SECONDS_PER_YEAR: U256 = uint!(31_536_000_U256);

    let elapsed = at
        .checked_sub(checkpoint_at)
        .ok_or(Error::CheckpointAhead {
            // Safe by the branch: `checked_sub` only gave `None` because
            // `checkpoint_at` is the larger.
            seconds: checkpoint_at.saturating_sub(at),
        })?;

    // Saturating rather than checked because 192 bits cannot reach 256, and
    // `SECONDS_PER_YEAR` is a nonzero constant. Neither can give out; the
    // crate's lint just will not take an unadorned operator for it.
    let interest = U256::from(rate)
        .saturating_mul(U256::from(elapsed))
        .wrapping_div(SECONDS_PER_YEAR);

    Ok(RAY.saturating_add(interest))
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
    fn refuses_a_product_where_the_contract_reverts() {
        // `rayMulUp`'s guard is `a <= type(uint256).max / b`, and it fires on
        // the product rather than on the result: `MAX * RAY / RAY` is `MAX` and
        // would fit, but the chain never gets there.
        assert_eq!(ray_mul_up(U256::MAX, RAY), Err(Error::OutOfRange));

        // One step under the guard, where it does answer.
        assert_eq!(ray_mul_up(U256::MAX / RAY, RAY), Ok(U256::MAX / RAY));
    }

    #[test]
    fn linear_interest_cannot_overflow_at_the_far_end_of_its_types() {
        // `uint96 × uint40` on chain, and its assembly has no guard because of
        // it. Even a full `u128` against a full `u64` — far past anything the
        // chain can emit — stays inside `uint256`.
        assert_eq!(
            U256::from(u128::MAX)
                .checked_mul(U256::from(u64::MAX))
                .map(|p| p.bit_len()),
            Some(192)
        );
        assert!(linear_interest(u128::MAX, 0, u64::MAX).is_ok());
    }
}
