import { join } from 'node:path';

import type { ClickHouseClient } from '@clickhouse/client';
import { Inject, Injectable } from '@nestjs/common';
import { CLICKHOUSE_CLIENT } from '@packages/clickhouse';

import type { Position, PositionAsset, PositionValue } from './position';
import { valuePosition, type AssetState } from '../valuation/valuation';
import { PositionCursorCodec, type CursorScope } from './position-cursor';
import type { PositionPage, PositionQuery, PositionStore } from './position-store';

const POSITIONS_VIEW = 'user_positions_current';
const RESERVES_VIEW = 'spoke_reserves_current';
const HUB_ASSETS_VIEW = 'hub_assets_current';

/**
 * This package's schema, owned here rather than in a central list.
 *
 * A directory rather than the loaded migrations: reading it is filesystem work,
 * and importing this package to use the store should not do any. The
 * application passes it to the runner at migration time, alongside the events
 * package's own — the ordinals are unique across both, and `010 > 002` is what
 * guarantees these views are created after the table they read.
 */
export const POSITION_MIGRATIONS_DIR = join(__dirname, 'clickhouse-migrations');

/** One row as ClickHouse renders it: every wide integer already a string. */
interface Row {
  readonly chain_id: number;
  readonly user: string;
  readonly spoke: string;
  readonly reserve_id: string;
  readonly supplied_shares: string;
  readonly drawn_shares: string;
  readonly premium_shares: string;
  readonly premium_offset_ray: string;
  readonly net_supplied_amount: string;
  readonly net_borrowed_amount: string;
  readonly using_as_collateral: 0 | 1;
  readonly events: number;

  // The joined halves. Null throughout when the reserve is not in the registry,
  // and null on the asset columns alone when the Hub has listed it but not yet
  // checkpointed it.
  readonly asset_id: string | null;
  readonly hub: string | null;
  readonly underlying: string | null;
  readonly decimals: number | null;
  readonly liquidity: string | null;
  readonly added_shares: string | null;
  readonly asset_drawn_shares: string | null;
  readonly swept: string | null;
  readonly asset_premium_shares: string | null;
  readonly asset_premium_offset_ray: string | null;
  readonly deficit_ray: string | null;
  readonly realized_fees: string | null;
  readonly liquidity_fee: number | null;
  readonly drawn_index: string | null;
  readonly drawn_rate: string | null;
  readonly checkpoint_at: string | null;
}

/** The Hub state a valuation needs, or null if any of it is missing. */
function toAssetState(row: Row): AssetState | null {
  if (row.drawn_index === null || row.checkpoint_at === null) return null;
  return {
    liquidity: BigInt(row.liquidity ?? '0'),
    addedShares: BigInt(row.added_shares ?? '0'),
    drawnShares: BigInt(row.asset_drawn_shares ?? '0'),
    swept: BigInt(row.swept ?? '0'),
    premiumShares: BigInt(row.asset_premium_shares ?? '0'),
    premiumOffsetRay: BigInt(row.asset_premium_offset_ray ?? '0'),
    deficitRay: BigInt(row.deficit_ray ?? '0'),
    realizedFees: BigInt(row.realized_fees ?? '0'),
    liquidityFee: BigInt(row.liquidity_fee ?? 0),
    checkpointIndex: BigInt(row.drawn_index),
    drawnRate: BigInt(row.drawn_rate ?? '0'),
    checkpointAt: BigInt(row.checkpoint_at),
  };
}

function toAsset(row: Row): PositionAsset | null {
  if (row.asset_id === null || row.hub === null) return null;
  if (row.underlying === null || row.decimals === null) return null;
  return {
    assetId: row.asset_id,
    hub: row.hub,
    underlying: row.underlying,
    decimals: row.decimals,
  };
}

function toValue(row: Row, at: bigint): PositionValue | null {
  const state = toAssetState(row);
  if (state === null) return null;

  const valued = valuePosition(
    {
      suppliedShares: BigInt(row.supplied_shares),
      drawnShares: BigInt(row.drawn_shares),
      premiumShares: BigInt(row.premium_shares),
      premiumOffsetRay: BigInt(row.premium_offset_ray),
    },
    state,
    at,
  );

  return {
    suppliedAmount: valued.suppliedAmount.toString(),
    drawnDebt: valued.drawnDebt.toString(),
    premiumDebt: valued.premiumDebt.toString(),
    totalDebt: valued.totalDebt.toString(),
    drawnIndex: valued.drawnIndex.toString(),
  };
}

function toPosition(row: Row, at: bigint): Position {
  return {
    chainId: row.chain_id,
    user: row.user,
    spoke: row.spoke,
    reserveId: row.reserve_id,
    suppliedShares: row.supplied_shares,
    drawnShares: row.drawn_shares,
    premiumShares: row.premium_shares,
    premiumOffsetRay: row.premium_offset_ray,
    netSuppliedAmount: row.net_supplied_amount,
    netBorrowedAmount: row.net_borrowed_amount,
    usingAsCollateral: row.using_as_collateral === 1,
    events: row.events,
    asset: toAsset(row),
    value: toValue(row, at),
  };
}

/**
 * Reads the folded positions out of ClickHouse.
 *
 * Two shapes worth explaining, because both are deliberate:
 *
 * **`chain_id`, `user` and `spoke` are all required and all bound**, which is
 * the whole sorting-key prefix above `reserve_id`. That is what makes a page a
 * seek into one wallet's contiguous rows rather than a filter over everyone's.
 *
 * **Every wide integer is `toString`-ed in SQL.** Not left to the JSON encoder's
 * defaults: a share balance past 2^53 that arrives as a number has already lost
 * its tail by the time it reaches this process (§7.5).
 */
@Injectable()
export class ClickHousePositionStore implements PositionStore {
  constructor(
    @Inject(CLICKHOUSE_CLIENT) private readonly client: ClickHouseClient,
    private readonly cursors: PositionCursorCodec,
  ) {}

  async list(query: PositionQuery): Promise<PositionPage> {
    // The listing this page belongs to. Signed alongside the resume point, so a
    // cursor cannot be carried across to another wallet, Spoke or chain.
    const scope: CursorScope = {
      chainId: query.chainId,
      user: query.user.toLowerCase(),
      spoke: query.spoke.toLowerCase(),
    };
    const params: Record<string, unknown> = {
      chainId: scope.chainId,
      user: scope.user,
      spoke: scope.spoke,
      // Fetch one more than asked. Its presence is what says there is a next
      // page; counting the whole result set to find out would defeat the point
      // of keyset paging.
      limit: query.limit + 1,
    };
    const filters = [
      // The full sorting-key prefix, so the scan starts at this wallet's rows
      // rather than filtering its way to them.
      `chain_id = {chainId:UInt32}`,
      `user = {user:String}`,
      `spoke = {spoke:String}`,
      // Deliberately `!= 0` rather than §12.1's `> 0`. Shares cannot go negative
      // on chain, so a negative fold is drift — and it should surface as a
      // visibly wrong number for §9 to catch, not vanish behind the filter that
      // hides closed positions.
      `(supplied_shares != 0 OR drawn_shares != 0)`,
    ];

    if (query.cursor !== undefined) {
      // Everything above `reserve_id` in the sorting key is already pinned by the
      // scope, so the resume point is one comparison on what is left.
      filters.push(`reserve_id > {afterReserve:UInt256}`);
      params['afterReserve'] = this.cursors.decode(query.cursor, scope);
    }

    const result = await this.client.query({
      query: `
        SELECT
            p.chain_id                      AS chain_id,
            p.user                          AS user,
            p.spoke                         AS spoke,
            toString(p.reserve_id)          AS reserve_id,
            toString(p.supplied_shares)     AS supplied_shares,
            toString(p.drawn_shares)        AS drawn_shares,
            toString(p.premium_shares)      AS premium_shares,
            toString(p.premium_offset_ray)  AS premium_offset_ray,
            toString(p.net_supplied_amount) AS net_supplied_amount,
            toString(p.net_borrowed_amount) AS net_borrowed_amount,
            p.using_as_collateral           AS using_as_collateral,
            toInt32(p.events)               AS events,
            toString(r.asset_id)            AS asset_id,
            r.hub                           AS hub,
            a.underlying                    AS underlying,
            a.decimals                      AS decimals,
            toString(a.liquidity)           AS liquidity,
            toString(a.added_shares)        AS added_shares,
            toString(a.drawn_shares)        AS asset_drawn_shares,
            toString(a.swept)               AS swept,
            toString(a.premium_shares)      AS asset_premium_shares,
            toString(a.premium_offset_ray)  AS asset_premium_offset_ray,
            toString(a.deficit_ray)         AS deficit_ray,
            toString(a.realized_fees)       AS realized_fees,
            a.liquidity_fee                 AS liquidity_fee,
            toString(a.drawn_index)         AS drawn_index,
            toString(a.drawn_rate)          AS drawn_rate,
            toString(toUnixTimestamp(a.index_timestamp)) AS checkpoint_at
        FROM (
            SELECT *
            FROM ${POSITIONS_VIEW}
            WHERE ${filters.join(' AND ')}
            ORDER BY user, spoke, reserve_id
            LIMIT {limit:UInt32}
        ) AS p
        -- **A join, not the UNION ALL the collateral flag got.** The two cases
        -- differ structurally, and EXPLAIN indexes = 1 shows how.
        --
        -- The left side prunes. Both branches of user_positions_current report
        -- PrimaryKey Keys: chain_id, user, spoke with the wallet predicate as
        -- their condition and Search Algorithm: binary search — that is the
        -- UNION ALL pushdown the flag was shaped for, doing its job.
        --
        -- The right sides do not, and cannot. All three read with
        -- PrimaryKey Condition: true — no predicate pushed in at all. The step
        -- is JOIN FillRightFirst: the right relation is built before a single
        -- left row is seen, so there is no key to filter by. A hash join
        -- materialises its whole right side by construction.
        --
        -- **UNION ALL would not change that.** Its advantage is exactly the
        -- pushdown above, and the Hub dimension has no predicate to push: it is
        -- keyed (chain, hub, asset_id), neither known until the registry
        -- resolves. It is not even expressible — UNION ALL needs one grouping
        -- key every branch shares, and the Hub dependency is transitive rather
        -- than co-keyed.
        --
        -- Reading the right side whole is fine when it *is* 17 rows, which is
        -- what the join was argued on. What is not fine is that producing those
        -- 17 reads 8 granules across 5 parts of hub_asset_state — the view
        -- collapses and argMaxes the lot on every page. Making that dimension a
        -- table is the fix; the README's "Not here yet" says how.
        --
        -- LEFT, because a position must survive a reserve the registry has not
        -- seen. The nulls that produces are reported as nulls rather than zeros.
        LEFT JOIN ${RESERVES_VIEW} AS r
               ON r.chain_id = p.chain_id AND r.spoke = p.spoke AND r.reserve_id = p.reserve_id
        LEFT JOIN ${HUB_ASSETS_VIEW} AS a
               ON a.chain_id = r.chain_id AND a.hub = r.hub AND a.asset_id = r.asset_id
        -- Qualified, and it has to be. Unqualified, \`reserve_id\` binds to the
        -- toString alias above and sorts the decimal digits as text, putting 13
        -- before 3 — which then hands the next page a cursor from the wrong row.
        ORDER BY p.reserve_id`,
      query_params: params,
      format: 'JSONEachRow',
    });

    const rows = await result.json<Row>();
    // One instant for the whole page, so two positions in one response cannot
    // disagree about what time it is — and reported back, because an amount
    // without the moment it was computed at is not reproducible (§12.6).
    const valuedAt = query.asOf ?? BigInt(Math.floor(Date.now() / 1000));
    const items = rows.slice(0, query.limit).map((row) => toPosition(row, valuedAt));
    const last = items.at(-1);

    return {
      items,
      valuedAt: Number(valuedAt),
      nextCursor:
        rows.length > query.limit && last !== undefined
          ? this.cursors.encode(scope, last.reserveId)
          : null,
    };
  }
}
