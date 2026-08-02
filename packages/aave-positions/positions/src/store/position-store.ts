import type { Address } from '@packages/indexing';

import type { Position } from './position';

/**
 * Where a page stopped: the sorting key below `(chain_id, user)`, and nothing
 * else.
 *
 * A plain key rather than an opaque token. Signing one so a caller cannot forge
 * or carry it between listings is a property of *publishing* it, not of paging,
 * and it lives with whoever publishes it — for the HTTP API, in `apps/api`.
 * Nothing in this package holds a signing key or knows what a cursor looks like.
 */
export interface PositionKey {
  readonly spoke: Address;
  readonly reserveId: string;
}

/**
 * One wallet's positions, on one Spoke or on all of them.
 *
 * **`user` is required**, which makes this a lookup rather than a scan: with
 * `chainId` it is the leading pair of the sorting key, so pinning it turns every
 * page into a seek into contiguous rows.
 *
 * **`spoke` is optional, and what it narrows is the listing, never the
 * arithmetic.** A Spoke is an isolated margin account with its own collateral
 * factors, oracle and health factor (§12.3), so *summing* across two of them is
 * wrong in the one direction that matters — it hides an imminent liquidation
 * behind unrelated collateral. Listing them together is fine, because every row
 * names the Spoke it came from. Nothing here is aggregated, and anything that
 * ever aggregates must do it per Spoke.
 *
 * Cross-wallet questions ("largest open positions") are analytics over the same
 * view, not a mode of this port.
 */
export interface PositionQuery {
  readonly chainId: number;
  /** Lower-cased by the store, so a checksummed address from a caller still matches. */
  readonly user: Address;
  /** Omitted lists every Spoke this wallet has touched. */
  readonly spoke?: Address;
  readonly limit: number;
  /** Resume point, already verified by whoever owns the wire format. */
  readonly after?: PositionKey;
  /**
   * Unix seconds to value the page at. Defaults to now.
   *
   * Amounts are a per-second quantity — the Hub's index accrues continuously
   * and emits nothing (§5) — so "now" is a real choice rather than an absence
   * of one, and it is the same choice the chain makes: `getUserDebt` at
   * `latest` is stored shares times an index extrapolated to the head block.
   * Naming it explicitly is what makes a response reproducible, and what lets
   * reconciliation pin both sides to one block.
   *
   * The shares are as of whatever the indexer has folded, which is not the same
   * clock. {@link PositionPage.valuedAt} reports this one so the two are not
   * silently conflated.
   */
  readonly asOf?: bigint;
}

export interface PositionPage {
  readonly items: readonly Position[];
  /**
   * Unix seconds every amount on this page was computed at — one instant for
   * the whole page, so two positions in it cannot disagree about the time.
   */
  readonly valuedAt: number;
  /** `null` on the last page. Absence is the end, not an empty key. */
  readonly next: PositionKey | null;
}

/**
 * Everything a wallet holds, unpaged.
 *
 * The same query {@link PositionQuery} describes, without the two fields that
 * make it a page — because a total over a page is arithmetic on a subset, and
 * wrong in a way that looks right.
 */
export interface PositionHoldingsQuery {
  readonly chainId: number;
  readonly user: Address;
  readonly spoke?: Address;
  readonly asOf?: bigint;
}

export interface PositionHoldings {
  readonly items: readonly Position[];
  readonly valuedAt: number;
}

/**
 * Reads the folded positions.
 *
 * Read-only by construction: nothing in this package writes. The fold is
 * materialized views over the event ledger, so positions advance because the
 * indexer appended events — there is no ingestion path here to keep in step
 * with one, and a reorg repairs the projection without this store learning of
 * it.
 *
 * **Only open positions.** §12.1: a position exists while its shares are
 * non-zero. A closed one keeps its row and its event count, and is filtered out
 * here rather than deleted anywhere.
 */
export interface PositionStore {
  list(query: PositionQuery): Promise<PositionPage>;

  /**
   * The same rows without paging, for a caller that has to see all of them at
   * once.
   *
   * **A second method rather than a `limit: Infinity`**, because the two have
   * different contracts and only one of them is safe to call casually. Paging
   * exists so a response cannot be unbounded; this deliberately gives that up,
   * and says so in its name.
   *
   * **Still not aggregated here.** It returns positions, each naming its own
   * Spoke, and leaves the summing to whoever knows the prices — which is not
   * this package. That keeps §12.3's rule enforceable at the place that can
   * enforce it, rather than baking one interpretation of "total" into SQL.
   *
   * Bounded in practice rather than by a `LIMIT`: `maxUserReservesLimit` from
   * `SetSpokeImmutables` caps a user's reserves on one Spoke, and a wallet can
   * only be on so many Spokes. The store refuses rather than truncates if that
   * stops holding — a short read here is a net worth that is quietly too small.
   */
  holdings(query: PositionHoldingsQuery): Promise<PositionHoldings>;
}

export const POSITION_STORE = Symbol('POSITION_STORE');
