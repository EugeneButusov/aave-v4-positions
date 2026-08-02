import type { ClickHouseClient } from '@clickhouse/client';
import { Inject, Injectable } from '@nestjs/common';
import { CLICKHOUSE_CLIENT } from '@packages/clickhouse';
import type { Address } from '@packages/indexing';

import type { HubAsset } from './hub-asset';
import type { HubAssetStore } from './hub-asset-store';

const HUB_ASSETS_VIEW = 'hub_assets_current';

/** One row as ClickHouse renders it: every wide integer already a string. */
interface Row {
  readonly chain_id: number;
  readonly hub: string;
  readonly asset_id: string;
  readonly liquidity: string;
  readonly added_shares: string;
  readonly drawn_shares: string;
  readonly swept: string;
  readonly premium_shares: string;
  readonly premium_offset_ray: string;
  readonly deficit_ray: string;
  readonly drawn_index: string | null;
  readonly drawn_rate: string | null;
  readonly realized_fees: string | null;
  readonly index_timestamp: string | null;
  readonly liquidity_fee: number | null;
  readonly underlying: string | null;
  readonly decimals: number | null;
  readonly events: number;
}

function toHubAsset(row: Row): HubAsset {
  return {
    chainId: row.chain_id,
    hub: row.hub,
    assetId: row.asset_id,
    liquidity: row.liquidity,
    addedShares: row.added_shares,
    drawnShares: row.drawn_shares,
    swept: row.swept,
    premiumShares: row.premium_shares,
    premiumOffsetRay: row.premium_offset_ray,
    deficitRay: row.deficit_ray,
    drawnIndex: row.drawn_index,
    drawnRate: row.drawn_rate,
    realizedFees: row.realized_fees,
    indexTimestamp: row.index_timestamp,
    liquidityFee: row.liquidity_fee,
    underlying: row.underlying,
    decimals: row.decimals,
    events: row.events,
  };
}

/**
 * The columns, named rather than `SELECT *`.
 *
 * `asset_id` is cast because it is a `UInt256`: ClickHouse renders one as a JSON
 * number by default and float64 would round it. Everything wide is already a
 * string by the time it reaches this process (§7.5).
 *
 * `index_timestamp` is formatted rather than left to the driver, so the caller
 * gets an unambiguous UTC instant instead of a locale-dependent rendering.
 */
const COLUMNS = `
    chain_id,
    hub,
    toString(asset_id)           AS asset_id,
    toString(liquidity)          AS liquidity,
    toString(added_shares)       AS added_shares,
    toString(drawn_shares)       AS drawn_shares,
    toString(swept)              AS swept,
    toString(premium_shares)     AS premium_shares,
    toString(premium_offset_ray) AS premium_offset_ray,
    toString(deficit_ray)        AS deficit_ray,
    toString(drawn_index)        AS drawn_index,
    toString(drawn_rate)         AS drawn_rate,
    toString(realized_fees)      AS realized_fees,
    formatDateTime(index_timestamp, '%Y-%m-%dT%H:%i:%SZ', 'UTC') AS index_timestamp,
    liquidity_fee,
    underlying,
    decimals,
    events
`;

/**
 * Reads the Hub asset fold out of ClickHouse.
 *
 * **Every asset the Hub has ever listed, including any whose balances net to
 * zero.** Unlike a position, an asset with no liquidity is still a real listing
 * with an interest index and a token address — the valuation path needs the row
 * to exist so a position against it can be valued at zero rather than dropped.
 * That is why there is no `sum(...) != 0` filter here where `PositionStore` has
 * one.
 */
@Injectable()
export class ClickHouseHubAssetStore implements HubAssetStore {
  constructor(@Inject(CLICKHOUSE_CLIENT) private readonly client: ClickHouseClient) {}

  async list(chainId: number, hub: Address): Promise<readonly HubAsset[]> {
    const result = await this.client.query({
      // **`ORDER BY a.asset_id`, qualified.** Unqualified it binds the
      // `toString(asset_id)` alias in the projection and sorts the text, which
      // puts 13 before 3 — the same bug the position store's pagination hit,
      // where it silently handed the next page a cursor from the wrong row.
      // Qualifying resolves to the source `UInt256` instead.
      query: `
        SELECT ${COLUMNS}
        FROM ${HUB_ASSETS_VIEW} AS a
        WHERE a.chain_id = {chainId:UInt32} AND a.hub = {hub:String}
        ORDER BY a.asset_id
      `,
      query_params: { chainId, hub: hub.toLowerCase() },
      format: 'JSONEachRow',
    });

    return (await result.json<Row>()).map(toHubAsset);
  }

  async get(chainId: number, hub: Address, assetId: string): Promise<HubAsset | null> {
    const result = await this.client.query({
      // Qualified for the same reason as `list`, and here the alias binding is
      // at least loud: unqualified, `asset_id` resolves to the `toString`
      // projection and comparing it to a `UInt256` parameter fails outright
      // rather than silently comparing text.
      query: `
        SELECT ${COLUMNS}
        FROM ${HUB_ASSETS_VIEW} AS a
        WHERE a.chain_id = {chainId:UInt32}
          AND a.hub = {hub:String}
          AND a.asset_id = {assetId:UInt256}
      `,
      query_params: { chainId, hub: hub.toLowerCase(), assetId },
      format: 'JSONEachRow',
    });

    const [row] = await result.json<Row>();
    return row ? toHubAsset(row) : null;
  }
}
