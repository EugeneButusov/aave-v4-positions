/**
 * Base units to a decimal string, exactly.
 *
 * The wire has carried integers in the protocol's own units since the endpoint
 * existed — `11290521655` for a WBTC balance, `6333703004688` for its price,
 * `715108109307684305186400000000000` for what that is worth. Every one of
 * those is correct and none of them is readable, and the scale that fixes them
 * is not the same scale twice: eight decimals for the token, eight for the
 * oracle, twenty-six for a `Value`, twenty-seven for a ray. A caller who gets
 * the pairing wrong is out by ten orders of magnitude with nothing to notice.
 *
 * **Strings in, strings out, and no `number` anywhere between.** `bigint` does
 * the arithmetic and the digits are sliced, so this is exact at any width. The
 * obvious version — `Number(v) / 10 ** decimals` — loses the tail above 2^53,
 * and share balances pass that routinely (§7.5): a real `Repay` in the fixture
 * carries 422,166,581,625,087,607,993.
 *
 * **Trailing zeros are trimmed**, so a whole number is `"112"` rather than
 * `"112.00000000"` and zero is `"0"`. The value is unchanged either way — this
 * is the same decision `numeric` makes on the way out of Postgres, and any
 * decimal parser reads both identically.
 *
 * @param value base units, as the chain stores them
 * @param decimals how many of its digits are fractional
 */
export function toDecimal(value: bigint, decimals: number): string {
  if (decimals <= 0) return value.toString();

  const negative = value < 0n;
  const digits = (negative ? -value : value).toString().padStart(decimals + 1, '0');

  const whole = digits.slice(0, -decimals);
  // Trailing zeros carry no information here, and keeping them would make the
  // string's shape depend on the token: `"1"` of an 18-decimal asset would
  // arrive with eighteen of them.
  const fraction = digits.slice(-decimals).replace(/0+$/, '');

  const sign = negative ? '-' : '';
  return fraction === '' ? `${sign}${whole}` : `${sign}${whole}.${fraction}`;
}

/** Convenience for the decimal strings the store already hands over. */
export function scale(value: string, decimals: number): string {
  return toDecimal(BigInt(value), decimals);
}

/**
 * How many digits of a ray are fractional.
 *
 * A fixed 27 rather than anything asset-derived: an index is a ratio, so its
 * scale is the protocol's and not the token's. Kept here beside the others so
 * the four scales this endpoint mixes are visible in one place.
 */
export const RAY_DECIMALS = 27;

/** `IAaveOracleV4.decimals()`, and §7.4's `ORACLE_DECIMALS`. */
export const ORACLE_DECIMALS = 8;

/**
 * How many digits of a protocol `Value` are fractional.
 *
 * §7.1 computes in a unit where `1e26` is one dollar — an 18-decimal-normalised
 * amount times an 8-decimal price. Twenty-six is that, and dividing by it here
 * is what turns the unit the contract reconciles in into the unit a reader
 * wants, without either side losing a digit.
 */
export const VALUE_DECIMALS = 26;
