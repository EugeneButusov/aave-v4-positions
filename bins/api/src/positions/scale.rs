//! Base units to a decimal string, exactly.
//!
//! The store hands back `U256` and `I256` — the fold's columns are 256-bit and
//! a JSON number has lost its tail before the process sees it (§7.5) — so the
//! wire format is decided here and nowhere else. Four scales meet on one page
//! and they are not the same scale twice: the asset's decimals for an amount,
//! eight for an oracle answer, twenty-six for a `Value`, twenty-seven for a
//! ray. A caller handed the wrong pairing is out by ten orders of magnitude
//! with nothing to notice.
//!
//! **Digits are sliced, never divided.** The obvious `value as f64 / 10f64.powi(n)`
//! loses everything past 2^53, and share balances pass that routinely: a real
//! `Repay` in the fixtures carries 422,166,581,625,087,607,993.
//!
//! **Trailing zeros are trimmed**, so a whole number is `"112"` rather than
//! `"112.00000000"` and zero is `"0"`. The value is unchanged either way — it
//! is the same decision `numeric` makes on the way out of Postgres, and any
//! decimal parser reads both identically.

use alloy_primitives::{I256, U256};

/// How many digits of a ray are fractional.
///
/// A fixed 27 rather than anything asset-derived: an index is a ratio, so its
/// scale is the protocol's and not the token's. Kept here beside the others so
/// the four scales this endpoint mixes are visible in one place.
pub(crate) const RAY_DECIMALS: u8 = 27;

/// `IAaveOracleV4.decimals()`, and §7.4's `ORACLE_DECIMALS`.
pub(crate) const ORACLE_DECIMALS: u8 = 8;

/// How many digits of a protocol `Value` are fractional.
///
/// §7.1 computes in a unit where `1e26` is one dollar — an 18-decimal-normalised
/// amount times an 8-decimal price. Twenty-six is that, and dividing by it here
/// is what turns the unit the contract reconciles in into the unit a reader
/// wants, without either side losing a digit.
pub(crate) const VALUE_DECIMALS: u8 = 26;

/// An unsigned quantity, with `decimals` of its digits fractional.
pub(crate) fn unsigned(value: U256, decimals: u8) -> String {
    place(value.to_string(), decimals)
}

/// A signed quantity, with the sign kept outside the digits.
///
/// The share columns are sums of `Int256` deltas and cannot go negative on
/// chain, so a negative one is drift — and §9 catches it by being able to see
/// it. Padding the signed string rather than its magnitude would render `-42`
/// at eight decimals as `-0.0000-42`, which is neither a number nor a signal.
pub(crate) fn signed(value: I256, decimals: u8) -> String {
    // `unsigned_abs` rather than negating: `I256::MIN` has no positive twin,
    // and this is the only reachable value where that matters.
    let digits = place(value.unsigned_abs().to_string(), decimals);

    if value.is_negative() {
        format!("-{digits}")
    } else {
        digits
    }
}

/// Puts the point `decimals` places from the right of an already-rendered
/// magnitude, padding on the left when the value is shorter than its own scale.
fn place(mut digits: String, decimals: u8) -> String {
    let decimals = usize::from(decimals);
    if decimals == 0 {
        // A zero-decimal asset must not acquire a point, and every slice below
        // would be taken from the wrong end.
        return digits;
    }

    if digits.len() <= decimals {
        let padding = decimals + 1 - digits.len();
        digits.insert_str(0, &"0".repeat(padding));
    }

    let (whole, fraction) = digits.split_at(digits.len() - decimals);
    let fraction = fraction.trim_end_matches('0');

    if fraction.is_empty() {
        whole.to_owned()
    } else {
        format!("{whole}.{fraction}")
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::uint;

    use super::*;

    fn at(value: u128, decimals: u8) -> String {
        unsigned(U256::from(value), decimals)
    }

    #[test]
    fn splits_the_digits_at_the_decimal_position() {
        assert_eq!(at(11_290_521_655, 8), "112.90521655");
    }

    #[test]
    fn pads_a_value_shorter_than_its_own_scale() {
        // An 8-decimal price of a dollar stablecoin: every digit is fractional,
        // and the integer part has to be produced rather than sliced.
        assert_eq!(at(99_988_821, ORACLE_DECIMALS), "0.99988821");
    }

    #[test]
    fn trims_trailing_zeros_and_the_point_with_them() {
        assert_eq!(at(11_200_000_000, 8), "112");
        assert_eq!(at(11_290_000_000, 8), "112.9");
    }

    #[test]
    fn renders_zero_as_zero_at_any_scale() {
        assert_eq!(at(0, 18), "0");
        assert_eq!(at(0, VALUE_DECIMALS), "0");
        assert_eq!(signed(I256::ZERO, RAY_DECIMALS), "0");
    }

    #[test]
    fn passes_an_unscaled_value_through_untouched() {
        assert_eq!(at(1234, 0), "1234");
        assert_eq!(signed(I256::unchecked_from(-1234), 0), "-1234");
    }

    #[test]
    fn is_exact_past_2_53_where_a_float_would_round() {
        // A real `Repay` from the fixtures.
        let shares = uint!(422_166_581_625_087_607_993_U256);
        let rendered = unsigned(shares, 18);

        assert_eq!(rendered, "422.166581625087607993");
        // And the tail really is what a float drops — writing the expectation
        // as a literal would not have caught anything, because the literal is
        // the rounded double too.
        assert_eq!(
            rendered.parse::<f64>().unwrap_or_default().to_string(),
            "422.1665816250876"
        );
    }

    #[test]
    fn keeps_every_digit_of_a_protocol_value() {
        // §7.1's unit, from the live endpoint: 1e26 is one dollar.
        let value = uint!(715_108_109_307_684_305_186_400_000_000_000_U256);

        assert_eq!(unsigned(value, VALUE_DECIMALS), "7151081.093076843051864");
    }

    #[test]
    fn renders_a_ray_as_the_ratio_it_is() {
        let index = uint!(1_001_135_055_846_810_133_967_161_790_U256);

        assert_eq!(
            unsigned(index, RAY_DECIMALS),
            "1.00113505584681013396716179"
        );
        assert_eq!(
            unsigned(
                uint!(1_000_000_000_000_000_000_000_000_000_U256),
                RAY_DECIMALS
            ),
            "1"
        );
    }

    #[test]
    fn keeps_the_sign_outside_the_digits() {
        // No negative amount reaches the wire today — a share column that went
        // negative would be drift (§9) — but padding the signed string rather
        // than its magnitude would produce `-0.0000-42`.
        assert_eq!(signed(I256::unchecked_from(-4_200_000_000_i64), 8), "-42");
        assert_eq!(signed(I256::MINUS_ONE, 8), "-0.00000001");
    }

    #[test]
    fn renders_the_one_value_that_cannot_be_negated() {
        // `-I256::MIN` overflows, so the magnitude is taken rather than the
        // negation. Unreachable from the fold, and the only input that could
        // panic here.
        assert_eq!(
            signed(I256::MIN, 0),
            "-57896044618658097711785492504343953926634992332820282019728792003956564819968"
        );
    }
}
