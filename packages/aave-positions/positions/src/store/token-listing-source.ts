import type { ClickHouseClient } from '@clickhouse/client';
import { Inject, Injectable } from '@nestjs/common';
import { CLICKHOUSE_CLIENT } from '@packages/clickhouse';
import type { Address } from '@packages/indexing';

/** Which ERC-20s the Hub has listed — the chain-side half of the gap query. */
export interface TokenListings {
  /** Every underlying the Hub has listed on this chain. */
  all(chainId: number): Promise<readonly Address[]>;
}

export const TOKEN_LISTINGS = Symbol('TOKEN_LISTINGS');

/**
 * Reads listed token addresses out of the Hub asset fold.
 *
 * **This looks like a full scan and is not, and the difference is worth knowing
 * before anyone tries to fix it.** `EXPLAIN indexes = 1` reports
 * `Parts: 2/2, Granules: 5/5` — every granule — because `underlying` is not in
 * the sorting key and `chain_id` prunes nothing on a single-chain deployment.
 * That reads like O(`UpdateAsset` history), which grows about 1.5 million rows
 * a year.
 *
 * Measured, it is not. `underlying` is NULL on every row except the handful of
 * `AddAsset`s, so ClickHouse stores the column sparsely and skips the NULL runs
 * rather than materialising them. At 3,029,631 rows — two years of history —
 * this query reads **34 rows and 1.83 KiB in 3 ms**, the same as it does at
 * 29,631. Granules *examined* is not data *read*.
 *
 * A purpose-built `(chain_id, underlying)` table fed by a materialized view was
 * built and measured against this: 17 rows, 816 bytes, 1 ms. It was reverted.
 * Two milliseconds does not buy a table, a view, a migration and a replay step —
 * and the replay is the real cost, because a materialized view does not see rows
 * written before it existed.
 *
 * What would change the answer is `underlying` ceasing to be sparse, which needs
 * a Hub listing assets in the same order of magnitude as it emits checkpoints.
 * At 17 assets against 434 `UpdateAsset` per 10,000 blocks, that is not close.
 */
@Injectable()
export class ClickHouseTokenListings implements TokenListings {
  constructor(@Inject(CLICKHOUSE_CLIENT) private readonly client: ClickHouseClient) {}

  async all(chainId: number): Promise<readonly Address[]> {
    const result = await this.client.query({
      // The event-grain table, not `hub_assets_current`. That view collapses and
      // `argMax`es all of `hub_asset_state` to produce 17 rows; discovery needs
      // only the set of addresses ever listed, and over-fetching a token whose
      // listing was later retracted costs one wasted ERC-20 read.
      //
      // **No `sign > 0`.** This is an append-only ledger, so a retraction is a
      // second row and the original `sign = +1` survives until a merge — the
      // filter would look like it excluded reorged listings without doing so.
      // Being lax is deliberate, and a spec pins it: over-fetching a rolled-back
      // token is a wasted call, while missing one leaves a position permanently
      // unlabelled.
      query: `
        SELECT DISTINCT underlying
        FROM hub_asset_state
        WHERE chain_id = {chainId:UInt32} AND underlying IS NOT NULL
      `,
      query_params: { chainId },
      format: 'JSONEachRow',
    });

    return (await result.json<{ underlying: Address }>())
      .map((row) => row.underlying)
      .filter((token) => token.length > 0);
  }
}
