import { describe, expect, it } from 'vitest';

import { ORACLE_DECIMALS, RAY_DECIMALS, VALUE_DECIMALS, scale, toDecimal } from './scale';

describe('toDecimal', () => {
  it('splits the digits at the decimal position', () => {
    expect(toDecimal(11_290_521_655n, 8)).toBe('112.90521655');
  });

  it('pads a value shorter than its own scale', () => {
    // 8-decimal price of a dollar stablecoin: every digit is fractional, and
    // the integer part has to be produced rather than sliced.
    expect(toDecimal(99_988_821n, ORACLE_DECIMALS)).toBe('0.99988821');
  });

  it('trims trailing zeros, and the point with them', () => {
    expect(toDecimal(11_200_000_000n, 8)).toBe('112');
    expect(toDecimal(11_290_000_000n, 8)).toBe('112.9');
  });

  it('renders zero as zero at any scale', () => {
    expect(toDecimal(0n, 18)).toBe('0');
    expect(toDecimal(0n, VALUE_DECIMALS)).toBe('0');
  });

  it('passes an unscaled value through untouched', () => {
    // Guards the `<= 0` branch: a zero-decimal asset must not acquire a point,
    // and `slice(-0)` would return the whole string rather than nothing.
    expect(toDecimal(1234n, 0)).toBe('1234');
  });

  it('is exact past 2^53, where a float would round', () => {
    // A real `Repay` from the fixtures. `Number(...) / 1e18` gives
    // 422.16658162508764 — the last four digits are gone.
    const shares = 422_166_581_625_087_607_993n;

    expect(toDecimal(shares, 18)).toBe('422.166581625087607993');
    // And the tail really is what a float drops — writing the expectation as a
    // numeric literal would not have caught anything, because the literal is
    // the rounded double too.
    expect(String(Number(toDecimal(shares, 18)))).toBe('422.1665816250876');
  });

  it('keeps every digit of a protocol Value', () => {
    // §7.1's unit, from the live endpoint: 1e26 is one dollar.
    expect(toDecimal(715_108_109_307_684_305_186_400_000_000_000n, VALUE_DECIMALS)).toBe(
      '7151081.093076843051864',
    );
  });

  it('renders a ray as the ratio it is', () => {
    expect(toDecimal(1_001_135_055_846_810_133_967_161_790n, RAY_DECIMALS)).toBe(
      '1.00113505584681013396716179',
    );
    expect(toDecimal(10n ** 27n, RAY_DECIMALS)).toBe('1');
  });

  it('keeps the sign outside the digits', () => {
    // No negative amount reaches the wire today — net worth is the first that
    // could — but the padding is done on the absolute value and a `-` slipping
    // into `padStart` would produce `-0.0000-42`.
    expect(toDecimal(-4_200_000_000n, 8)).toBe('-42');
    expect(toDecimal(-1n, 8)).toBe('-0.00000001');
  });

  it('reads the decimal strings the store hands over', () => {
    expect(scale('1870292269286637679847938', 18)).toBe('1870292.269286637679847938');
  });
});
