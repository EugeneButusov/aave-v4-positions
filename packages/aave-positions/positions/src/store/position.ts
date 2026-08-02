import type { Address } from '@packages/indexing';

/**
 * What a reserve refers to, once the registry and the Hub have both been read.
 *
 * `reserveId` is a per-Spoke index and means nothing on its own (§1). This is
 * the resolved identity: `AddReserve` supplies the Hub and its asset id, and
 * the Hub's `AddAsset` supplies the ERC-20 and its decimals — no Spoke event
 * carries the token address.
 */
export interface PositionAsset {
  readonly assetId: string;
  readonly hub: Address;
  readonly underlying: Address;
  readonly decimals: number;
}

/**
 * Balances in token units, at {@link PositionPage.valuedAt}.
 *
 * Not stored — computed on read, because the index accrues every second and
 * emits nothing (§5). The arithmetic is §5.1 and §5.2, transcribed from the
 * contracts, and validated against `getUserSuppliedAssets` / `getUserDebt` for
 * real mainnet positions at 36 of 36 exact.
 */
export interface PositionValue {
  /** Underlying redeemable for the supplied shares, rounded down as the Hub does. */
  readonly suppliedAmount: string;
  /** Rounded up, as the Spoke does. */
  readonly drawnDebt: string;
  readonly premiumDebt: string;
  /** `drawnDebt + premiumDebt` — what `getUserTotalDebt` returns. */
  readonly totalDebt: string;
  /** The index this valuation used: the checkpoint extrapolated to `valuedAt`. */
  readonly drawnIndex: string;
}

/**
 * One user's stake in one reserve on one Spoke.
 *
 * **Shares are the stored truth; {@link value} is derived.** Between events the
 * Hub's index accrues and emits nothing, so a share balance is not a balance
 * (§5) — the amounts are computed at read time from the Hub asset mirror and
 * carry the timestamp they were computed at.
 *
 * {@link asset} and {@link value} are **null together**, and only when the join
 * has nothing to offer: a reserve the registry has not seen, or a Hub asset with
 * no interest checkpoint yet. Reporting zero there would be a number the caller
 * cannot distinguish from a real one.
 *
 * Every `uint256` is a decimal string. Share balances run past 2^53 routinely —
 * a real `Repay` in the fixture carries 422,166,581,625,087,607,993 — and
 * float64 would silently round the tail (§7.5).
 */
export interface Position {
  readonly chainId: number;
  /** Lower-case. §12.1 keys on `user`, never the `caller` that routed the call. */
  readonly user: Address;
  /** The Spoke this position lives on. Two Spokes are two independent positions. */
  readonly spoke: Address;
  /** The Spoke's own key for the reserve. Resolved to a token by {@link asset}. */
  readonly reserveId: string;

  readonly suppliedShares: string;
  readonly drawnShares: string;
  readonly premiumShares: string;
  readonly premiumOffsetRay: string;

  /**
   * Net principal flow in asset units — what was put in less what came out.
   *
   * Kept beside {@link value} rather than replaced by it, because the two answer
   * different questions: this one is cost basis, and the difference between them
   * is interest.
   */
  readonly netSuppliedAmount: string;
  readonly netBorrowedAmount: string;

  /**
   * The user's own collateral flag, and only that.
   *
   * §12.1's `collateral` type additionally requires `collateralFactor > 0` under
   * the user's pinned `dynamicConfigKey`, which needs config events this
   * increment does not ingest. Five of the Main Spoke's fourteen reserves have
   * `CF = 0`, so this flag alone overstates collateral for those.
   */
  readonly usingAsCollateral: boolean;

  /** Ledger rows folded into this position. Zero shares with a non-zero count is a closed position. */
  readonly events: number;

  /** Null when the registry has not seen this reserve — see the note above. */
  readonly asset: PositionAsset | null;
  /** Null when {@link asset} is, or when the Hub asset has no checkpoint yet. */
  readonly value: PositionValue | null;
}
