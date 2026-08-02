/**
 * The protocol's fixed-point primitives, transcribed from `aave/aave-v4` at
 * commit `2524fe4`.
 *
 * **Rounding is not uniform and the directions are not interchangeable.**
 * Reconciliation is at zero tolerance (§9.2) — any nonzero drift is a bug, not
 * noise — so each of these reproduces one specific Solidity function and is
 * named after it rather than after what it does.
 *
 * All of them take non-negative inputs. BigInt division truncates toward zero,
 * which equals floor only for non-negative operands; every caller here is
 * `uint256` on chain, so that equality holds and no sign handling is needed.
 * {@link premiumRay} is where a negative could arise, and it throws for the
 * same reason the contract reverts.
 */

/** `MathUtils.RAY` — 1e27. */
export const RAY = 10n ** 27n;

/** `MathUtils.SECONDS_PER_YEAR` — 365 days, leap years ignored. */
export const SECONDS_PER_YEAR = 365n * 24n * 60n * 60n;

/** `SharesMath.VIRTUAL_ASSETS` and `VIRTUAL_SHARES`, both 1e6. */
export const VIRTUAL = 10n ** 6n;

/** `PercentageMath.PERCENTAGE_FACTOR` — basis points. */
export const PERCENTAGE_FACTOR = 10_000n;

/** `WadRayMath.rayMulUp` — `ceil(a * b / RAY)`. */
export function rayMulUp(a: bigint, b: bigint): bigint {
  const product = a * b;
  return product / RAY + (product % RAY > 0n ? 1n : 0n);
}

/** `WadRayMath.fromRayUp` — `ceil(a / RAY)`. */
export function fromRayUp(a: bigint): bigint {
  return a / RAY + (a % RAY > 0n ? 1n : 0n);
}

/** `PercentageMath.percentMulDown` — `floor(value * bps / 10000)`. */
export function percentMulDown(value: bigint, bps: bigint): bigint {
  return (value * bps) / PERCENTAGE_FACTOR;
}

/** `Math.mulDiv(..., Rounding.Floor)`, as `SharesMath.toAssetsDown` uses it. */
export function mulDivDown(a: bigint, b: bigint, denominator: bigint): bigint {
  return (a * b) / denominator;
}

/**
 * `MathUtils.calculateLinearInterest` — `RAY + rate * elapsed / SECONDS_PER_YEAR`.
 *
 * Integer division, so the interest term floors before `RAY` is added. Reverts
 * on chain if the checkpoint is in the future; the same here, because a
 * checkpoint ahead of the valuation time means the caller mixed up two blocks
 * and every number that follows would be quietly wrong.
 */
export function linearInterest(rate: bigint, elapsedSeconds: bigint): bigint {
  if (elapsedSeconds < 0n) {
    throw new RangeError(`checkpoint is ${-elapsedSeconds}s ahead of the valuation time`);
  }
  return RAY + (rate * elapsedSeconds) / SECONDS_PER_YEAR;
}

/**
 * `Premium.calculatePremiumRay` — `premiumShares * drawnIndex - premiumOffsetRay`.
 *
 * `premiumOffsetRay` is `int200` on chain and genuinely negative, so the
 * subtraction can go either way. The contract closes it with `.toUint256()`,
 * which **reverts** on a negative — so a negative premium is not a state the
 * protocol can be in, and producing one here would mean the mirror is wrong
 * rather than that the maths needs a signed branch. Throwing keeps that
 * distinction visible instead of returning a number nothing can interpret.
 */
export function premiumRay(
  premiumShares: bigint,
  premiumOffsetRay: bigint,
  drawnIndex: bigint,
): bigint {
  const value = premiumShares * drawnIndex - premiumOffsetRay;
  if (value < 0n) {
    throw new RangeError(
      `premium is negative (${value}): shares ${premiumShares}, offset ${premiumOffsetRay}, index ${drawnIndex}`,
    );
  }
  return value;
}
