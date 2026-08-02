import type { Address } from '@packages/indexing';

import type { ReservePrice, ReservePriceRow } from './reserve-price';

/**
 * The key both sides of the join agree on.
 *
 * Exported rather than inlined at each end, because a price is keyed by
 * `(spoke, reserveId)` and the read path holds those two as separate fields —
 * so the moment the store and the service each spell the key themselves is the
 * moment one of them forgets to lower-case the spoke and every price silently
 * stops joining.
 */
export function reserveKey(spoke: Address, reserveId: string): string {
  return `${spoke.toLowerCase()}:${reserveId}`;
}

/**
 * Reads and writes what Aave prices each reserve at.
 *
 * The second store here that this codebase writes rather than a materialized
 * view — see {@link TokenMetadataStore} for the first, and
 * `011_reserve_prices.sql` for why both are in Postgres.
 *
 * **No pagination**, for the reason {@link HubAssetStore} gives: a Spoke lists
 * fourteen reserves on mainnet, the whole dimension fits in one response, and
 * the read path wants all of it at once.
 */
export interface ReservePriceStore {
  /**
   * Every price on this chain, keyed by {@link reserveKey}.
   *
   * **The whole dimension, deliberately not the reserves on a page.** Taking a
   * page's keys would make this depend on the page and force it to run after
   * the ClickHouse query; keyed only by chain it runs *beside* it, in the same
   * `Promise.all` the labels read already uses — a third round trip that costs
   * no wall clock.
   *
   * A reserve with no row is **absent from the map**, and the read path serves
   * null for it rather than a zero. A price of zero is not a price: the oracle
   * reverts on one (§7.4), so a zero here could only ever be our own invention.
   */
  latest(chainId: number): Promise<ReadonlyMap<string, ReservePrice>>;

  /**
   * Upserts whole rows. Re-running with the same input changes only `priced_at`.
   *
   * **A reserve the oracle refused is simply absent from `rows`**, never present
   * with a null. Skipping it leaves the last good price and its timestamp in
   * place, so a provider outage shows up as an ageing price rather than as a
   * blank — and `pricing.stale` is what makes that visible.
   */
  put(rows: readonly ReservePriceRow[]): Promise<void>;
}

export const RESERVE_PRICE_STORE = Symbol('RESERVE_PRICE_STORE');
