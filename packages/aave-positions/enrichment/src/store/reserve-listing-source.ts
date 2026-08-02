import type { ClickHouseClient } from '@clickhouse/client';
import { Inject, Injectable } from '@nestjs/common';
import { CLICKHOUSE_CLIENT } from '@packages/clickhouse';
import type { Address } from '@packages/indexing';

/** Which reserves a Spoke has listed — the set the oracle is asked about. */
export interface ReserveListings {
  /**
   * Every `reserveId` this Spoke has registered, as decimal strings.
   *
   * Ordered, so a batch call and the spec that pins it both see one arrangement
   * rather than whatever the parts happened to merge into.
   */
  forSpoke(chainId: number, spoke: Address): Promise<readonly string[]>;
}

export const RESERVE_LISTINGS = Symbol('RESERVE_LISTINGS');

/**
 * Reads listed reserve ids out of the Spoke registry fold.
 *
 * **A prefix seek, unlike {@link ClickHouseTokenListings}.** That one scans
 * every granule because `underlying` is not a sorting key and gets away with it
 * only because the column is sparse. This binds `chain_id` and `spoke`, which
 * are the first two components of `spoke_reserves`' `ORDER BY`, so the read is
 * pruned rather than merely cheap — and it stays that way as history grows.
 *
 * Reads the resolved view rather than the ledger, which is the opposite of what
 * token discovery does and is right for the opposite reason. Over-fetching a
 * token whose listing was retracted costs one wasted ERC-20 read; over-fetching
 * a *reserve* costs an oracle call that reverts, because a reserve the Spoke no
 * longer knows about has no price source — and `getReservesPrices` is a batch,
 * so one such id would take the other thirteen down with it.
 */
@Injectable()
export class ClickHouseReserveListings implements ReserveListings {
  constructor(@Inject(CLICKHOUSE_CLIENT) private readonly client: ClickHouseClient) {}

  async forSpoke(chainId: number, spoke: Address): Promise<readonly string[]> {
    const result = await this.client.query({
      // Aliased, and the `ORDER BY` has to be. Unqualified, `reserve_id` binds
      // to the `toString` alias above it and sorts the decimal digits as text,
      // putting 13 before 3 — the same trap `clickhouse-position-store.ts`
      // documents, where it silently handed the next page a key from the wrong
      // row. Here it would only reorder a batch, but it would reorder it
      // differently from the spec that pins it.
      query: `
        SELECT toString(r.reserve_id) AS reserve_id
        FROM spoke_reserves_current AS r
        WHERE r.chain_id = {chainId:UInt32} AND r.spoke = {spoke:String}
        ORDER BY r.reserve_id
      `,
      query_params: { chainId, spoke: spoke.toLowerCase() },
      format: 'JSONEachRow',
    });

    return (await result.json<{ reserve_id: string }>()).map((row) => row.reserve_id);
  }
}
