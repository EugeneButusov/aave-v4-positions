import type { Address } from '@packages/indexing';

import type { TokenLabel, TokenMetadataRow } from './token-metadata';

/**
 * Reads and writes what tokens call themselves.
 *
 * The only store here that is written by this codebase rather than by a
 * materialized view. Everything else in this package is a projection the
 * database maintains; this is fetched, so it needs a `put`.
 *
 * **No pagination**, for the reason {@link HubAssetStore} gives: a Hub lists 17
 * assets on mainnet, the whole dimension fits in one response, and the read
 * path wants all of it at once.
 */
export interface TokenMetadataStore {
  /**
   * Every label on this chain, keyed by lower-cased address.
   *
   * **The whole dimension, deliberately not the tokens on a page.** Taking a
   * page's addresses would make this depend on the page and force it to run
   * after the ClickHouse query; keyed only by chain it runs *beside* it, which
   * is what makes the second round trip free — measured at 0.27 ms against a
   * 28 ms page. It is also the stored half of the gap query, so the enrichment
   * sweep reads it too.
   *
   * A token with no row is **absent from the map**, not present with nulls: the
   * caller has to tell "never asked" from "asked, and it has no symbol",
   * because one is a gap to fill and the other is not.
   */
  labels(chainId: number): Promise<ReadonlyMap<Address, TokenLabel>>;

  /** Upserts whole rows. Re-running with the same input changes nothing. */
  put(rows: readonly TokenMetadataRow[]): Promise<void>;
}

export const TOKEN_METADATA_STORE = Symbol('TOKEN_METADATA_STORE');
