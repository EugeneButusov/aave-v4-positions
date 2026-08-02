import type { Address } from '@packages/indexing';

/**
 * One user's stake in one reserve on one Spoke.
 *
 * **Balances are shares, not assets.** Converting them needs the Hub's interest
 * index, and no Hub event is ingested yet — so this carries what the Spoke's own
 * logs prove and nothing inferred. {@link netSuppliedAmount} is the closest
 * thing to a human number available today, and it is a *flow*, not a balance:
 * between events the index accrues and emits nothing (§5).
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
  readonly reserveId: string;

  /**
   * The reserve's asset within its Hub, from `AddReserve`.
   *
   * `null` when the registry has no entry — which happens legitimately while a
   * backfill is mid-flight and has not yet reached the `AddReserve` log. The
   * underlying ERC-20 address is deliberately absent: it is on the Hub's
   * `AddAsset`, not on any Spoke event, so it arrives with Hub ingestion.
   */
  readonly assetId: string | null;
  readonly hub: Address | null;

  readonly suppliedShares: string;
  readonly drawnShares: string;
  readonly premiumShares: string;
  readonly premiumOffsetRay: string;

  /** Net principal flow in asset units. Not a balance — see the note above. */
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
}
