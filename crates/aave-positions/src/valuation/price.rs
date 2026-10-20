//! `SpokeUtils.toValue` — an amount in token units, priced.

use alloy_primitives::{U256, U512};

use super::Error;
use super::math::narrow;
use crate::scale::WAD_DECIMALS;

/// `SpokeUtils.toValue` — an amount in token units, priced.
///
/// The result is the protocol's own Value unit, not dollars: 26 decimal places,
/// where `1e26` is one dollar (§7.1, `SpokeUtils.toValue:28-40`). Served in
/// that unit because it is the only form that reconciles against
/// `getUserAccountData`; a caller slices 26 digits rather than dividing (§7.5).
///
/// `decimals` is the Hub's, from `AddAsset`, never the token's own `decimals()`.
pub fn to_value(amount: U256, decimals: u8, price: U256) -> Result<U256, Error> {
    let product: U512 = amount.widening_mul(price);

    // The contract writes `10 ** (18 - dec)` and would revert past eighteen
    // decimals, which no listed asset has. Dividing continues the same
    // arithmetic rather than taking a page down with a hypothetical listing.
    let scaled = if decimals <= WAD_DECIMALS {
        let exponent = U512::from(u32::from(WAD_DECIMALS).saturating_sub(u32::from(decimals)));
        let scale = U512::from(10)
            .checked_pow(exponent)
            .ok_or(Error::OutOfRange)?;
        product.checked_mul(scale).ok_or(Error::OutOfRange)?
    } else {
        let exponent = U512::from(u32::from(decimals).saturating_sub(u32::from(WAD_DECIMALS)));
        match U512::from(10).checked_pow(exponent) {
            Some(scale) => product.wrapping_div(scale),
            // A divisor past `U512::MAX` exceeds any dividend this can hold, so
            // the floor is exactly zero.
            None => U512::ZERO,
        }
    };

    narrow(scaled)
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use alloy_primitives::uint;

    use super::*;

    mod to_value_of {
        use super::*;

        /// ETH/USD as §7.4.2 records it, 8 decimals.
        const ETH_USD: U256 = uint!(187_522_000_000_U256);
        /// USDC/USD as §7.4.3 records it.
        const USDC_USD: U256 = uint!(99_971_505_U256);

        /// One dollar of Value — `1e26`.
        const ONE_DOLLAR: U256 = uint!(100_000_000_000_000_000_000_000_000_U256);

        /// An oracle answer of exactly $1.00, at `ORACLE_DECIMALS = 8`.
        ///
        /// Named for the price it is rather than the value it produces:
        /// [`ONE_DOLLAR`] is also "one dollar" and is 1e26.
        const A_DOLLAR_PER_TOKEN: U256 = uint!(100_000_000_U256);

        #[test]
        fn puts_one_dollar_at_1e26() {
            //   1e6 (one USDC, 6 dp) × 1e8 ($1.00, 8 dp) × 1e12 (6 dp → 18 dp)
            assert_eq!(
                to_value(U256::from(1_000_000), 6, A_DOLLAR_PER_TOKEN),
                Ok(ONE_DOLLAR)
            );
        }

        #[test]
        fn normalises_the_amount_to_eighteen_decimals_not_the_price() {
            // Taken from the wrong side, these would differ by 1e12.
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

            assert_eq!(value / ONE_DOLLAR, U256::from(1_875));
        }

        #[test]
        fn keeps_the_digits_a_division_would_lose() {
            // 1000 USDC at $0.99971505 is $999.71505, not an integer number of
            // dollars — the point of publishing the raw Value.
            let value = to_value(U256::from(1_000_000_000), 6, USDC_USD).unwrap();

            assert_eq!(value, USDC_USD * uint!(1_000_000_000_000_000_000_000_U256));
            assert_eq!(value / ONE_DOLLAR, U256::from(999));
        }

        #[test]
        fn is_exact_past_2_53() {
            // float64 would round the tail off both operands (§7.5).
            let amount = uint!(422166581625087607993_U256);

            assert_eq!(to_value(amount, 18, ETH_USD), Ok(amount * ETH_USD));
        }

        #[test]
        fn divides_rather_than_reverting_past_eighteen_decimals() {
            assert_eq!(
                to_value(
                    uint!(100_000_000_000_000_000_000_U256),
                    20,
                    A_DOLLAR_PER_TOKEN
                ),
                Ok(ONE_DOLLAR)
            );
        }

        #[test]
        fn is_zero_for_a_zero_amount_and_only_for_one() {
            assert_eq!(to_value(U256::ZERO, 6, ETH_USD), Ok(U256::ZERO));
            assert!(to_value(U256::from(1), 6, ETH_USD).unwrap() > U256::ZERO);
        }

        #[test]
        fn floors_to_zero_rather_than_refusing_an_absurd_decimals() {
            // 10^237 is past `U512::MAX`, and also past any dividend this can
            // hold, so the floor is exact.
            assert_eq!(to_value(U256::MAX, 255, U256::MAX), Ok(U256::ZERO));
        }
    }
}
