import type { ClickHouseClient } from '@clickhouse/client';
import { Inject, Injectable } from '@nestjs/common';
import { CLICKHOUSE_CLIENT } from '@packages/clickhouse';
import type { Address } from '@packages/indexing';

/** Which ERC-20s the Hub has listed — the chain-side half of the gap query. */
export interface TokenListings {
  /** Every underlying the Hub has ever listed on this chain. */
  all(chainId: number): Promise<readonly Address[]>;

  /** The underlyings named by an `AddAsset` inside one block range. */
  addedIn(chainId: number, from: number, to: number): Promise<readonly Address[]>;
}

export const TOKEN_LISTINGS = Symbol('TOKEN_LISTINGS');

/**
 * Reads listed token addresses out of the Hub ledger.
 *
 * Two queries, because enrichment has two jobs and they want different things.
 *
 * {@link addedIn} is the per-range fast path. `hub_events` is
 * `ORDER BY (chain_id, block_number, log_index)` and partitioned by block, so
 * binding the chain and a block range is a granule-pruned seek rather than a
 * scan — cheap enough to run on every dispatch, and it costs no RPC because
 * the Hub processor has already written the row.
 *
 * {@link all} is the sweep. It exists because the fast path cannot bootstrap:
 * all 17 `AddAsset` fired at block 24,722,784, far behind any live cursor, so
 * a freshly started indexer would never see one.
 *
 * **Neither reads `hub_assets_current`**, and that is deliberate. The view
 * collapses and `argMax`es all of `hub_asset_state` to produce 17 rows;
 * discovery needs only the set of addresses ever listed, which is the handful
 * of rows where `underlying` is not null. Over-fetching a token whose listing
 * was later retracted costs one row nobody joins to — so skipping the collapse
 * is safe as well as cheaper.
 */
@Injectable()
export class ClickHouseTokenListings implements TokenListings {
  constructor(@Inject(CLICKHOUSE_CLIENT) private readonly client: ClickHouseClient) {}

  async all(chainId: number): Promise<readonly Address[]> {
    const result = await this.client.query({
      query: `
        SELECT DISTINCT underlying
        FROM hub_asset_state
        WHERE chain_id = {chainId:UInt32} AND underlying IS NOT NULL
      `,
      query_params: { chainId },
      format: 'JSONEachRow',
    });

    return (await result.json<{ underlying: Address }>()).map((row) => row.underlying);
  }

  async addedIn(chainId: number, from: number, to: number): Promise<readonly Address[]> {
    const result = await this.client.query({
      // Read from the ledger rather than re-fetching the logs: the Hub
      // processor runs earlier in the same dispatch and has already written
      // them, so this costs a seek instead of a second `eth_getLogs`.
      //
      // `DISTINCT`, not `LIMIT 17`. Nothing rules out two asset ids sharing an
      // underlying, and a fixed count would be wrong the day one does.
      //
      // **No collapse, and no `sign > 0` either.** This is an append-only
      // ledger, so a retraction is a second row rather than a deletion and the
      // original `sign = +1` stays until a merge — a `sign > 0` filter looks
      // like it excludes reorged listings and does not, which is worse than
      // not filtering. Being lax is the deliberate choice, the same one
      // {@link all} makes: over-fetching a token whose listing was rolled back
      // writes one row nothing joins to, while missing one leaves a position
      // permanently unlabelled.
      query: `
        SELECT DISTINCT lower(JSONExtractString(body, 'underlying')) AS underlying
        FROM hub_events
        WHERE chain_id = {chainId:UInt32}
          AND block_number BETWEEN {from:UInt64} AND {to:UInt64}
          AND event_name = 'AddAsset'
      `,
      query_params: { chainId, from, to },
      format: 'JSONEachRow',
    });

    return (await result.json<{ underlying: Address }>())
      .map((row) => row.underlying)
      .filter((token) => token.length > 0);
  }
}
