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
   * Labels for the tokens on a page, keyed by lower-cased address.
   *
   * A token with no row is **absent from the map**, not present with nulls —
   * the caller has to be able to tell "never asked" from "asked, and it has no
   * symbol", because those render differently and one of them is a gap to fill.
   */
  labels(chainId: number, tokens: readonly Address[]): Promise<ReadonlyMap<Address, TokenLabel>>;

  /** Every token this chain has a row for. The stored half of the gap query. */
  known(chainId: number): Promise<ReadonlySet<Address>>;

  /** Upserts whole rows. Re-running with the same input changes nothing. */
  put(rows: readonly TokenMetadataRow[]): Promise<void>;
}

export const TOKEN_METADATA_STORE = Symbol('TOKEN_METADATA_STORE');
