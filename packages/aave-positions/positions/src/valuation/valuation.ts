import {
  RAY,
  VIRTUAL,
  fromRayUp,
  linearInterest,
  mulDivDown,
  percentMulDown,
  premiumRay,
  rayMulUp,
} from './ray';

/**
 * The Hub asset state one valuation reads, as bigints.
 *
 * A narrowed view of `HubAsset` — the fields the arithmetic touches and nothing
 * else, so a change to the row shape cannot silently alter a formula.
 */
export interface AssetState {
  readonly liquidity: bigint;
  readonly addedShares: bigint;
  readonly drawnShares: bigint;
  readonly swept: bigint;
  readonly premiumShares: bigint;
  readonly premiumOffsetRay: bigint;
  readonly deficitRay: bigint;
  readonly realizedFees: bigint;
  readonly liquidityFee: bigint;
  /** The last `UpdateAsset`'s index — the checkpoint, not the current value. */
  readonly checkpointIndex: bigint;
  readonly drawnRate: bigint;
  /** Unix seconds. `accrue()` sets it to the checkpoint block's timestamp. */
  readonly checkpointAt: bigint;
}

/** One user's shares in one reserve. */
export interface PositionShares {
  readonly suppliedShares: bigint;
  readonly drawnShares: bigint;
  readonly premiumShares: bigint;
  readonly premiumOffsetRay: bigint;
}

export interface Valuation {
  /** Underlying redeemable for `suppliedShares`, rounded down as the Hub does. */
  readonly suppliedAmount: bigint;
  readonly drawnDebt: bigint;
  readonly premiumDebt: bigint;
  /** `drawnDebt + premiumDebt`, which is what `getUserTotalDebt` returns. */
  readonly totalDebt: bigint;
  /** The index the valuation used — the checkpoint extrapolated to `at`. */
  readonly drawnIndex: bigint;
}

/**
 * `AssetLogic.getDrawnIndex`, extrapolated to an arbitrary time.
 *
 * The index accrues every second and emits nothing (§5), so a balance is only
 * meaningful with a time attached. What makes this exact rather than a model is
 * §5.3: the Hub emits the *settled* index on every accrual, so this applies
 * linear interest to an authoritative checkpoint instead of reconstructing a
 * rate curve.
 *
 * **Two short-circuits, both from the contract and both load-bearing.** The
 * index does not move when the checkpoint is already at `at`, nor when the
 * asset owes nothing at all — and the second is on the *asset's* totals, not
 * the user's, so a user with debt in an otherwise-idle asset still does not
 * accrue. Skipping either inflates every debt in that asset.
 */
export function drawnIndexAt(asset: AssetState, at: bigint): bigint {
  if (asset.checkpointAt === at) return asset.checkpointIndex;
  if (asset.drawnShares === 0n && asset.premiumShares === 0n) return asset.checkpointIndex;

  return rayMulUp(asset.checkpointIndex, linearInterest(asset.drawnRate, at - asset.checkpointAt));
}

/** `AssetLogic._calculateAggregatedOwedRay` — everything the asset is owed, RAY-scaled. */
function aggregatedOwedRay(asset: AssetState, drawnIndex: bigint): bigint {
  return (
    asset.drawnShares * drawnIndex +
    premiumRay(asset.premiumShares, asset.premiumOffsetRay, drawnIndex) +
    asset.deficitRay
  );
}

/**
 * `AssetLogic.getUnrealizedFees` — the protocol's cut of interest accrued since
 * the checkpoint, not yet folded into `realizedFees`.
 *
 * This is the one term §5.2 states in outline rather than in full, so it is
 * transcribed rather than reconstructed. Note it is a difference of two
 * *separately rounded* values — `fromRayUp(after) - fromRayUp(before)` — and not
 * `fromRayUp(after - before)`, which would differ by a wei whenever both have a
 * remainder.
 */
function unrealizedFees(asset: AssetState, drawnIndex: bigint): bigint {
  if (asset.checkpointIndex === drawnIndex) return 0n;
  if (asset.liquidityFee === 0n) return 0n;

  const after = fromRayUp(aggregatedOwedRay(asset, drawnIndex));
  const before = fromRayUp(aggregatedOwedRay(asset, asset.checkpointIndex));
  return percentMulDown(after - before, asset.liquidityFee);
}

/**
 * `AssetLogic.totalAddedAssets` — what the whole supply side is worth.
 *
 * Suppliers are paid out of accrued debt, so this depends on `drawnIndex` and
 * therefore on time: the supply side is a per-second quantity too, not just the
 * debt side (§5.2).
 */
export function totalAddedAssets(asset: AssetState, drawnIndex: bigint): bigint {
  return (
    asset.liquidity +
    asset.swept +
    fromRayUp(aggregatedOwedRay(asset, drawnIndex)) -
    asset.realizedFees -
    unrealizedFees(asset, drawnIndex)
  );
}

/**
 * `SharesMath.toAssetsDown`, via `previewRemoveByShares` — the supply side.
 *
 * **Not an index.** ERC-4626 virtual assets and shares of `1e6` each pad the
 * ratio against manipulation, and the padding is inside the division rather
 * than applied after it, so it cannot be factored out. Rounds **down**, where
 * the debt side rounds up.
 */
export function suppliedAssets(shares: bigint, asset: AssetState, drawnIndex: bigint): bigint {
  return mulDivDown(
    shares,
    totalAddedAssets(asset, drawnIndex) + VIRTUAL,
    asset.addedShares + VIRTUAL,
  );
}

/**
 * One position's balances at `at`, in token units.
 *
 * `at` is Unix seconds and is the caller's choice, exactly as `block.timestamp`
 * is the chain's: `getUserDebt` at `latest` is stored shares times an index
 * extrapolated to the head block, and this is the same computation with the
 * time named rather than implied.
 *
 * The debt side rounds **up** throughout — `rayMulUp` on the drawn part and
 * `fromRayUp` on the premium, each separately, which is what `Spoke.getUserDebt`
 * does before summing. Rounding the total once instead would be a wei light.
 */
export function valuePosition(position: PositionShares, asset: AssetState, at: bigint): Valuation {
  const drawnIndex = drawnIndexAt(asset, at);

  const drawnDebt = rayMulUp(position.drawnShares, drawnIndex);
  const premiumDebt = fromRayUp(
    premiumRay(position.premiumShares, position.premiumOffsetRay, drawnIndex),
  );

  return {
    suppliedAmount: suppliedAssets(position.suppliedShares, asset, drawnIndex),
    drawnDebt,
    premiumDebt,
    totalDebt: drawnDebt + premiumDebt,
    drawnIndex,
  };
}

/**
 * One dollar, in the unit {@link toValue} answers in.
 *
 * Exported so a caller dividing by it says what it is doing. Not applied here:
 * the division is lossy and the wire carries the exact integer (§7.5).
 */
export const USD = 10n ** 26n;

/**
 * `SpokeUtils.toValue` — an amount in token units, priced.
 *
 * **The unit is the protocol's own and is not dollars.** It is an
 * 18-decimal-normalised amount times an 8-decimal price, so `1e26` represents
 * one dollar — `SpokeUtils.toValue:28-40` documents that outright, against
 * `ORACLE_DECIMALS = 8` (§7.1). Served in that unit rather than converted,
 * because it is what the contract computes in and therefore the only form that
 * reconciles against `getUserAccountData`.
 *
 * `decimals` is the **Hub's**, from `AddAsset`, and never the token's own
 * `decimals()`. The two can disagree — which is a listing audit signal, not a
 * correctness problem — and when they do, the Hub's is what the Hub's
 * arithmetic uses and so what the position is worth to Aave.
 */
export function toValue(amount: bigint, decimals: number, price: bigint): bigint {
  // The contract writes `10 ** (18 - dec)` and would revert on a token with
  // more than eighteen decimals, which no listed asset has. Continuing the same
  // arithmetic by dividing is not a different rule, and it keeps a hypothetical
  // listing from taking a whole page down with it.
  return decimals <= 18
    ? amount * price * 10n ** BigInt(18 - decimals)
    : (amount * price) / 10n ** BigInt(decimals - 18);
}

export { RAY };
