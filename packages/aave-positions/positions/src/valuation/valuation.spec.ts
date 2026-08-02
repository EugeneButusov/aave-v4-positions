import { describe, expect, it } from 'vitest';

import { RAY, VIRTUAL, fromRayUp, percentMulDown, premiumRay, rayMulUp } from './ray';
import {
  USD,
  drawnIndexAt,
  suppliedAssets,
  totalAddedAssets,
  toValue,
  valuePosition,
  type AssetState,
  type PositionShares,
} from './valuation';

const HOUR = 3600n;
const YEAR = 365n * 24n * HOUR;
/** 5% per annum, RAY-scaled, as `drawnRate` arrives on `UpdateAsset`. */
const FIVE_PERCENT = RAY / 20n;
const T0 = 1_785_000_000n;

function asset(over: Partial<AssetState> = {}): AssetState {
  return {
    liquidity: 1_000_000n,
    addedShares: 1_000_000n,
    drawnShares: 400_000n,
    swept: 0n,
    premiumShares: 0n,
    premiumOffsetRay: 0n,
    deficitRay: 0n,
    realizedFees: 0n,
    liquidityFee: 0n,
    checkpointIndex: RAY,
    drawnRate: FIVE_PERCENT,
    checkpointAt: T0,
    ...over,
  };
}

const position = (over: Partial<PositionShares> = {}): PositionShares => ({
  suppliedShares: 0n,
  drawnShares: 0n,
  premiumShares: 0n,
  premiumOffsetRay: 0n,
  ...over,
});

describe('the fixed-point primitives', () => {
  it('rounds a ray product up, and only when there is a remainder', () => {
    expect(rayMulUp(2n, RAY)).toBe(2n);
    expect(rayMulUp(1n, RAY + 1n)).toBe(2n);
    expect(rayMulUp(0n, RAY)).toBe(0n);
  });

  it('rounds a ray division up, and only when there is a remainder', () => {
    expect(fromRayUp(RAY)).toBe(1n);
    expect(fromRayUp(RAY + 1n)).toBe(2n);
    expect(fromRayUp(1n)).toBe(1n);
    expect(fromRayUp(0n)).toBe(0n);
  });

  it('rounds a percentage down', () => {
    // The one place the protocol rounds against the protocol. 9999 bps of 1 is
    // zero, not one.
    expect(percentMulDown(1n, 9_999n)).toBe(0n);
    expect(percentMulDown(10_000n, 1_000n)).toBe(1_000n);
  });

  it('refuses a negative premium instead of inventing one', () => {
    // `premiumOffsetRay` is int200 and really goes negative, so the subtraction
    // can go either way — but the contract closes it with `.toUint256()`, which
    // reverts. A negative here means the fold is wrong, not that the formula
    // needs a signed branch.
    expect(() => premiumRay(1n, 2n * RAY, RAY)).toThrow(/premium is negative/);
    expect(premiumRay(1n, -1n, RAY)).toBe(RAY + 1n);
  });
});

describe('the drawn index', () => {
  it('applies linear interest from the checkpoint', () => {
    // 5% for exactly a year, on an index of 1.0.
    expect(drawnIndexAt(asset(), T0 + YEAR)).toBe(RAY + FIVE_PERCENT);
  });

  it('compounds only where a checkpoint landed', () => {
    // Two years in one step is 10%, not 10.25% — interest is linear between
    // checkpoints and compounds only when one lands (§5.1).
    expect(drawnIndexAt(asset(), T0 + 2n * YEAR)).toBe(RAY + 2n * FIVE_PERCENT);
  });

  it('does not move at the checkpoint itself', () => {
    expect(drawnIndexAt(asset(), T0)).toBe(RAY);
  });

  it('does not accrue on an asset that owes nothing', () => {
    // The short-circuit is on the *asset's* totals. Dropping it makes every
    // idle asset's index creep upward and inflates any debt later drawn.
    const idle = asset({ drawnShares: 0n, premiumShares: 0n });

    expect(drawnIndexAt(idle, T0 + YEAR)).toBe(RAY);
  });

  it('still accrues when only the premium is outstanding', () => {
    // `drawnShares == 0 && premiumShares == 0` — both, not either.
    const premiumOnly = asset({ drawnShares: 0n, premiumShares: 1n });

    expect(drawnIndexAt(premiumOnly, T0 + YEAR)).toBeGreaterThan(RAY);
  });

  it('refuses a checkpoint in the future', () => {
    // On chain this reverts. Here it means two different blocks were mixed up,
    // and every number downstream would be quietly wrong.
    expect(() => drawnIndexAt(asset(), T0 - 1n)).toThrow(/ahead of the valuation time/);
  });
});

describe('debt', () => {
  it('rounds each component up separately, as the Spoke does', () => {
    // getUserDebt returns (rayMulUp(shares, index), fromRayUp(premiumRay)) and
    // getUserTotalDebt sums those two already-rounded numbers. Rounding the sum
    // once instead is a wei light.
    const state = asset({ checkpointIndex: RAY + 1n, premiumShares: 1n });
    const held = position({ drawnShares: 1n, premiumShares: 1n });

    const { drawnDebt, premiumDebt, totalDebt } = valuePosition(held, state, T0);

    expect(drawnDebt).toBe(2n);
    expect(premiumDebt).toBe(2n);
    expect(totalDebt).toBe(4n);
  });

  it('grows with time on a fixed share balance', () => {
    const held = position({ drawnShares: 1_000_000n });

    const now = valuePosition(held, asset(), T0).totalDebt;
    const later = valuePosition(held, asset(), T0 + YEAR).totalDebt;

    // The whole reason a share balance is not a balance (§5).
    expect(now).toBe(1_000_000n);
    expect(later).toBe(1_050_000n);
  });

  it('is zero for a position with no debt', () => {
    expect(valuePosition(position(), asset(), T0 + YEAR).totalDebt).toBe(0n);
  });
});

describe('supply', () => {
  it('pads the ratio with virtual assets and shares', () => {
    // 1e6 each, inside the division rather than applied after — the padding
    // cannot be factored out, and dropping it changes every result.
    const state = asset({ liquidity: 0n, addedShares: 0n, drawnShares: 0n, premiumShares: 0n });

    // An empty asset: shares redeem 1:1 against the padding alone.
    expect(suppliedAssets(500n, state, RAY)).toBe(500n);
    expect(VIRTUAL).toBe(1_000_000n);
  });

  it('rounds down, where debt rounds up', () => {
    const state = asset({ liquidity: 1n, addedShares: 3n, drawnShares: 0n, premiumShares: 0n });

    // (1 * (1 + 1e6)) / (3 + 1e6) floors to 0 rather than rounding to 1.
    expect(suppliedAssets(1n, state, RAY)).toBe(0n);
  });

  it('counts swept liquidity and the amount owed, and subtracts settled fees', () => {
    const state = asset({
      liquidity: 1_000n,
      swept: 500n,
      drawnShares: 2_000n,
      realizedFees: 300n,
      premiumShares: 0n,
      premiumOffsetRay: 0n,
    });

    // liquidity + swept + owed − realizedFees = 1000 + 500 + 2000 − 300.
    expect(totalAddedAssets(state, RAY)).toBe(3_200n);
  });

  it('carries bad debt as if still owed', () => {
    // §12.3: a deficit stays inside aggregatedOwedRay until eliminated, so
    // suppliers hold shares partly backed by it rather than taking a haircut.
    const withDeficit = asset({ deficitRay: 100n * RAY, drawnShares: 0n, premiumShares: 0n });
    const without = asset({ drawnShares: 0n, premiumShares: 0n });

    expect(totalAddedAssets(withDeficit, RAY) - totalAddedAssets(without, RAY)).toBe(100n);
  });

  it('takes the protocol cut out of interest accrued since the checkpoint', () => {
    const state = asset({
      drawnShares: 1_000_000n,
      liquidityFee: 1_000n, // 10%
      premiumShares: 0n,
      premiumOffsetRay: 0n,
    });

    const atCheckpoint = totalAddedAssets(state, RAY);
    const aYearOn = totalAddedAssets(state, drawnIndexAt(state, T0 + YEAR));

    // 50,000 of interest accrues; the protocol takes 10% and suppliers get the
    // other 45,000.
    expect(aYearOn - atCheckpoint).toBe(45_000n);
  });

  it('rounds each side of the fee difference separately', () => {
    // `fromRayUp(after) - fromRayUp(before)`, not `fromRayUp(after - before)`.
    // The two agree unless both sides have a remainder and the later one's is
    // larger, which is what this constructs: owed goes from RAY+1 to 2·RAY+2,
    // so the separately-rounded difference is 3−2 = 1 while rounding the
    // difference once gives ceil((RAY+1)/RAY) = 2.
    const state = asset({
      liquidity: 0n,
      swept: 0n,
      realizedFees: 0n,
      drawnShares: 1n,
      premiumShares: 0n,
      premiumOffsetRay: 0n,
      deficitRay: 1n,
      liquidityFee: 10_000n, // 100%, so the fee cannot round the gap away
      checkpointIndex: RAY,
    });

    // owed(RAY) = RAY + 1 → 2;  owed(2·RAY+1) = 2·RAY + 2 → 3;  fee = 1.
    expect(totalAddedAssets(state, 2n * RAY + 1n)).toBe(3n - 1n);
  });

  it('charges no unrealized fee when the index has not moved', () => {
    const state = asset({ drawnShares: 1_000_000n, liquidityFee: 1_000n });

    // Both short-circuits in getUnrealizedFees: same index, and zero fee.
    expect(totalAddedAssets(state, RAY)).toBe(
      totalAddedAssets({ ...state, liquidityFee: 0n }, RAY),
    );
  });

  it('rises for a supplier as debt accrues underneath them', () => {
    const state = asset({ drawnShares: 1_000_000n, addedShares: 1_000_000n });
    const held = position({ suppliedShares: 1_000_000n });

    const now = valuePosition(held, state, T0).suppliedAmount;
    const later = valuePosition(held, state, T0 + YEAR).suppliedAmount;

    // The supply side is a per-second quantity too, because it is paid out of
    // the debt side (§5.2).
    expect(later).toBeGreaterThan(now);
  });
});

describe('values past 2^53', () => {
  it('stays exact on a realistic share balance', () => {
    // A real Repay in the Spoke fixture carries this many shares, and float64
    // would round the tail (§7.5).
    const held = position({ drawnShares: 422_166_581_625_087_607_993n });
    const state = asset({ checkpointIndex: RAY, drawnShares: 422_166_581_625_087_607_993n });

    expect(valuePosition(held, state, T0).drawnDebt).toBe(422_166_581_625_087_607_993n);
  });
});

describe('toValue', () => {
  /** ETH/USD as §7.4.2 records it, 8 decimals. */
  const ETH_USD = 187_522_000_000n;
  /** USDC/USD as §7.4.3 records it. */
  const USDC_USD = 99_971_505n;

  it('puts one dollar at 1e26', () => {
    // `SpokeUtils.toValue` documents the unit outright: an 18-decimal amount
    // times an 8-decimal price. One whole USDC at exactly $1 is one dollar.
    expect(toValue(1_000_000n, 6, 100_000_000n)).toBe(USD);
  });

  it('normalises the amount to eighteen decimals, not the price', () => {
    // The same dollar value from two tokens with different decimals. If the
    // exponent were taken from the wrong side these would differ by 1e12.
    const oneUsdcWorth = toValue(1_000_000n, 6, 100_000_000n);
    const oneDaiWorth = toValue(10n ** 18n, 18, 100_000_000n);

    expect(oneUsdcWorth).toBe(oneDaiWorth);
  });

  it('values a whole-token amount at its price', () => {
    // 1 WETH at $1875.22.
    expect(toValue(10n ** 18n, 18, ETH_USD) / USD).toBe(1_875n);
  });

  it('keeps the digits a division would lose', () => {
    // 1000 USDC at $0.99971505 is $999.71505, which is not representable as an
    // integer number of dollars — the point of publishing the raw Value.
    const value = toValue(1_000_000_000n, 6, USDC_USD);

    expect(value).toBe(99_971_505n * 10n ** 21n);
    expect(value / USD).toBe(999n);
  });

  it('is exact past 2^53', () => {
    // A realistic share-scale amount priced. float64 would round the tail off
    // both operands long before the product mattered (§7.5).
    expect(toValue(422_166_581_625_087_607_993n, 18, ETH_USD)).toBe(
      422_166_581_625_087_607_993n * ETH_USD,
    );
  });

  it('divides rather than reverting past eighteen decimals', () => {
    // The contract writes `10 ** (18 - dec)` and would revert here. No listed
    // asset has more than eighteen, so this is about not taking a whole page
    // down with a hypothetical listing — the arithmetic continued, not a
    // different rule.
    expect(toValue(10n ** 20n, 20, 100_000_000n)).toBe(USD);
  });

  it('is zero for a zero amount, and only for one', () => {
    expect(toValue(0n, 6, ETH_USD)).toBe(0n);
    expect(toValue(1n, 6, ETH_USD)).toBeGreaterThan(0n);
  });
});
