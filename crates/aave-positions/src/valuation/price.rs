//! `SpokeUtils.toValue` — an amount in token units, priced.
//!
//! Its own file because it shares no input with the rest of the valuation: it
//! takes an amount, the Hub's decimals and an oracle price, and never sees an
//! `AssetState`. It is also the one function `bins/api` calls directly.

use alloy_primitives::{U256, U512, uint};

use super::Error;
use super::math::narrow;

/// One dollar, in the unit [`to_value`] answers in.
///
/// Exported so a caller dividing by it says what it is doing. Not applied here:
/// the division is lossy and the wire carries the exact integer (§7.5).
pub const USD: U256 = uint!(100_000_000_000_000_000_000_000_000_U256);

/// `SpokeUtils.toValue` — an amount in token units, priced.
///
/// **The unit is the protocol's own and is not dollars.** It is an
/// 18-decimal-normalised amount times an 8-decimal price, so `1e26` represents
/// one dollar — `SpokeUtils.toValue:28-40` documents that outright, against
/// `ORACLE_DECIMALS = 8` (§7.1). Served in that unit rather than converted,
/// because it is what the contract computes in and therefore the only form that
/// reconciles against `getUserAccountData`.
///
/// `decimals` is the **Hub's**, from `AddAsset`, and never the token's own
/// `decimals()`. The two can disagree — which is a listing audit signal, not a
/// correctness problem — and when they do, the Hub's is what the Hub's
/// arithmetic uses and so what the position is worth to Aave.
pub fn to_value(amount: U256, decimals: u8, price: U256) -> Result<U256, Error> {
    // `SpokeUtils.toValue` is `amount * price * 10 ** (18 - dec)` in plain
    // checked Solidity, so it reverts on the first multiply. Widening changes
    // no answer on that path — the scale is at least 1, so a product past
    // `uint256` makes the result past it too, and both refuse the same inputs.
    // It is the `else` branch that needs the room, where a division follows and
    // an intermediate over `uint256` can still come back under it.
    let product: U512 = amount.widening_mul(price);

    // The contract writes `10 ** (18 - dec)` and would revert on a token with
    // more than eighteen decimals, which no listed asset has. Continuing the
    // same arithmetic by dividing is not a different rule, and it keeps a
    // hypothetical listing from taking a whole page down with it.
    let scaled = if decimals <= 18 {
        let exponent = U512::from(18u32.saturating_sub(u32::from(decimals)));
        let scale = U512::from(10)
            .checked_pow(exponent)
            .ok_or(Error::OutOfRange)?;
        product.checked_mul(scale).ok_or(Error::OutOfRange)?
    } else {
        let exponent = U512::from(u32::from(decimals).saturating_sub(18));
        match U512::from(10).checked_pow(exponent) {
            Some(scale) => product.wrapping_div(scale),
            // A divisor past `U512::MAX` exceeds any dividend this can hold, so
            // the floor is zero. Exact, not a fallback.
            None => U512::ZERO,
        }
    };

    narrow(scaled)
}

// Same reason as `math`'s: the arithmetic in a vector is the vector, and ruint
// panics on overflow in every profile, so a fixture that does not fit fails the
// test.
#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use super::*;

    mod to_value_of {
        use super::*;

        /// ETH/USD as §7.4.2 records it, 8 decimals.
        const ETH_USD: U256 = uint!(187_522_000_000_U256);
        /// USDC/USD as §7.4.3 records it.
        const USDC_USD: U256 = uint!(99_971_505_U256);

        /// An oracle answer of exactly $1.00, at `ORACLE_DECIMALS = 8`.
        ///
        /// Named for the price it is, not the value it produces: `USD` is also
        /// "one dollar" and is 1e26. Both appear in the assertion below, which
        /// is the point of it — a dollar going in does not look like a dollar
        /// coming out.
        const A_DOLLAR_PER_TOKEN: U256 = uint!(100_000_000_U256);

        #[test]
        fn puts_one_dollar_at_1e26() {
            // `SpokeUtils.toValue` documents the unit outright: an 18-decimal
            // amount times an 8-decimal price. One whole USDC genuinely worth
            // one dollar is what pins `USD` to 1e26 rather than leaving it a
            // number someone chose:
            //
            //   1e6 (one USDC, 6 dp) × 1e8 ($1.00, 8 dp) × 1e12 (6 dp → 18 dp)
            assert_eq!(
                to_value(U256::from(1_000_000), 6, A_DOLLAR_PER_TOKEN),
                Ok(USD)
            );
        }

        #[test]
        fn normalises_the_amount_to_eighteen_decimals_not_the_price() {
            // The same dollar value from two tokens with different decimals. If
            // the exponent were taken from the wrong side these would differ by
            // 1e12.
            let one_usdc_worth = to_value(U256::from(1_000_000), 6, A_DOLLAR_PER_TOKEN);
            let one_dai_worth = to_value(
                uint!(1_000_000_000_000_000_000_U256),
                18,
                A_DOLLAR_PER_TOKEN,
            );

            assert_eq!(one_usdc_worth, one_dai_worth);
        }

        #[test]
        fn values_a_whole_token_amount_at_its_price() {
            // 1 WETH at $1875.22.
            let value = to_value(uint!(1_000_000_000_000_000_000_U256), 18, ETH_USD).unwrap();

            assert_eq!(value / USD, U256::from(1_875));
        }

        #[test]
        fn keeps_the_digits_a_division_would_lose() {
            // 1000 USDC at $0.99971505 is $999.71505, which is not
            // representable as an integer number of dollars — the point of
            // publishing the raw Value.
            let value = to_value(U256::from(1_000_000_000), 6, USDC_USD).unwrap();

            assert_eq!(value, USDC_USD * uint!(1_000_000_000_000_000_000_000_U256));
            assert_eq!(value / USD, U256::from(999));
        }

        #[test]
        fn is_exact_past_2_53() {
            // A realistic share-scale amount priced. float64 would round the
            // tail off both operands long before the product mattered (§7.5).
            let amount = uint!(422166581625087607993_U256);

            assert_eq!(to_value(amount, 18, ETH_USD), Ok(amount * ETH_USD));
        }

        #[test]
        fn divides_rather_than_reverting_past_eighteen_decimals() {
            // The contract writes `10 ** (18 - dec)` and would revert here. No
            // listed asset has more than eighteen, so this is about not taking
            // a whole page down with a hypothetical listing — the arithmetic
            // continued, not a different rule.
            assert_eq!(
                to_value(
                    uint!(100_000_000_000_000_000_000_U256),
                    20,
                    A_DOLLAR_PER_TOKEN
                ),
                Ok(USD)
            );
        }

        #[test]
        fn is_zero_for_a_zero_amount_and_only_for_one() {
            assert_eq!(to_value(U256::ZERO, 6, ETH_USD), Ok(U256::ZERO));
            assert!(to_value(U256::from(1), 6, ETH_USD).unwrap() > U256::ZERO);
        }

        #[test]
        fn floors_to_zero_rather_than_refusing_an_absurd_decimals() {
            // 10^237 is past `U512::MAX`, so there is no divisor to build — but
            // it also exceeds any dividend this can hold, which makes the floor
            // exactly zero. Reachable only from a `u8` the chain would have to
            // have emitted.
            assert_eq!(to_value(U256::MAX, 255, U256::MAX), Ok(U256::ZERO));
        }
    }
}
