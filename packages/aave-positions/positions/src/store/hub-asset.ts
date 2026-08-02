import type { Address } from '@packages/indexing';

/**
 * One Hub asset's state, folded from the Hub ledger.
 *
 * This is the other half of a balance. A position carries *shares*; turning
 * them into token amounts needs these numbers, because debt accrues with time
 * through `drawnIndex` and that accrual emits no log (§5).
 *
 * **Nothing here is per-user, and nothing here is per-Spoke.** The Hub keeps
 * these totals asset-globally and user shares divide against them directly,
 * with no per-spoke scaling (§5.2b) — which is why the fold sums every Spoke's
 * events into one row.
 *
 * Every `uint256` is a decimal string. `drawnIndex` is RAY-scaled, so it starts
 * at 1e27 and is past 2^53 from the first block; float64 would round the tail
 * and every debt valuation with it (§7.5).
 */
export interface HubAsset {
  readonly chainId: number;
  /** The Hub holding it. Asset ids are Hub-scoped, so this is part of the key. */
  readonly hub: Address;
  readonly assetId: string;

  /** Underlying the Hub holds. Excludes anything swept to a reinvestment controller. */
  readonly liquidity: string;
  readonly addedShares: string;
  readonly drawnShares: string;
  /** Sent out to reinvestment, and still counted in `totalAddedAssets` (§5.2). */
  readonly swept: string;

  readonly premiumShares: string;
  /** `int200` on chain, and genuinely negative — hence a signed decimal string. */
  readonly premiumOffsetRay: string;
  /** Bad debt, RAY-scaled. Stays inside the owed aggregate until eliminated (§12.3). */
  readonly deficitRay: string;

  /**
   * The interest checkpoint, and the reason valuation needs no RPC (§5.3).
   *
   * `drawnIndex` is the *settled* index as of `indexTimestamp`, not a stale one:
   * `accrue()` writes it and `lastUpdateTimestamp` before `updateDrawnRate`
   * reads it to emit. Extrapolating to any later `t` is linear in `drawnRate`,
   * so no interest-rate model is needed anywhere.
   *
   * Null only before the asset's first `UpdateAsset`, which on mainnet is the
   * same transaction that lists it.
   */
  readonly drawnIndex: string | null;
  readonly drawnRate: string | null;
  readonly indexTimestamp: string | null;
  /** Fees already settled to the protocol. Zeroed when minted as shares. */
  readonly realizedFees: string | null;

  /** Basis points, out of 10,000. The fee rate in the supply side's fee term (§5.2). */
  readonly liquidityFee: number | null;

  /** The ERC-20, lower-cased. From `AddAsset`, which is its only source (§12.2). */
  readonly underlying: Address | null;
  readonly decimals: number | null;

  /** Ledger rows folded into the additive half. Latest-wins events are not counted. */
  readonly events: number;
}
