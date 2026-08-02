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
 * Reads the set of listed ERC-20s.
 *
 * **The obvious query, made cheap by the table rather than by rewriting it.**
 * `DISTINCT` over `underlying` is a full column scan on its own — the column is
 * not in the sorting key, and `chain_id` prunes nothing on a single-chain
 * deployment — and the cost grows with `UpdateAsset` history at about 1.5
 * million rows a year.
 *
 * `hub_asset_state` carries a `listed_tokens` projection for exactly this.
 * Measured on the same 1,000,017 rows with it on and off: **18 rows and 948
 * bytes** against 1,000,017 rows and 8.85 MiB. The plan says why — on the
 * projection `underlying` is the second key column, so
 * `Condition: chain_id in [1, 1] AND underlying isNotNull` prunes as an index
 * condition rather than filtering after the read.
 *
 * A projection is chosen by the optimizer, which is why nothing here had to
 * change and why nothing here should: rewriting the query is how it stops
 * matching. A spec asserts `system.query_log` names the projection, because
 * otherwise nothing would notice if it stopped being used.
 *
 * Called rarely in any case — once at start, and after a run that left a gap
 * open. The steady state is not a query at all: newly listed tokens are pushed
 * by the processor that ingests the `AddAsset`.
 */
@Injectable()
export class ClickHouseTokenListings implements TokenListings {
  constructor(@Inject(CLICKHOUSE_CLIENT) private readonly client: ClickHouseClient) {}

  async all(chainId: number): Promise<readonly Address[]> {
    const result = await this.client.query({
      // Read at event grain, not through `hub_assets_current`: that view
      // collapses and `argMax`es the whole table to produce 17 rows, where this
      // wants only the set of addresses ever listed.
      //
      // **No `sign > 0`.** This is an append-only ledger, so a retraction is a
      // second row and the original `sign = +1` survives until a merge — the
      // filter would look like it excluded reorged listings without doing so.
      // Being lax is deliberate, and a spec pins it: over-fetching a
      // rolled-back token is one wasted ERC-20 read, while missing one leaves a
      // position permanently unlabelled.
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
