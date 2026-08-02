import { join } from 'node:path';

import type { ClickHouseClient } from '@clickhouse/client';
import { Inject, Injectable } from '@nestjs/common';
import { CLICKHOUSE_CLIENT } from '@packages/clickhouse';

import type { Position } from './position';
import { decodeCursor, encodeCursor } from './position-cursor';
import type { PositionPage, PositionQuery, PositionStore } from './position-store';

export const POSITIONS_VIEW = 'user_positions_current';
export const RESERVES_VIEW = 'spoke_reserves_current';

/**
 * This package's schema, owned here rather than in a central list.
 *
 * A directory rather than the loaded migrations: reading it is filesystem work,
 * and importing this package to use the store should not do any. The
 * application passes it to the runner at migration time, alongside the events
 * package's own — the ordinals are unique across both, and `010 > 002` is what
 * guarantees these views are created after the table they read.
 */
export const POSITION_MIGRATIONS_DIR = join(__dirname, 'migrations');

/** One row as ClickHouse renders it: every wide integer already a string. */
interface Row {
  readonly chain_id: number;
  readonly user: string;
  readonly spoke: string;
  readonly reserve_id: string;
  readonly asset_id: string | null;
  readonly hub: string | null;
  readonly supplied_shares: string;
  readonly drawn_shares: string;
  readonly premium_shares: string;
  readonly premium_offset_ray: string;
  readonly net_supplied_amount: string;
  readonly net_borrowed_amount: string;
  readonly using_as_collateral: 0 | 1;
  readonly events: number;
}

function toPosition(row: Row): Position {
  return {
    chainId: row.chain_id,
    user: row.user,
    spoke: row.spoke,
    reserveId: row.reserve_id,
    assetId: row.asset_id,
    hub: row.hub,
    suppliedShares: row.supplied_shares,
    drawnShares: row.drawn_shares,
    premiumShares: row.premium_shares,
    premiumOffsetRay: row.premium_offset_ray,
    netSuppliedAmount: row.net_supplied_amount,
    netBorrowedAmount: row.net_borrowed_amount,
    usingAsCollateral: row.using_as_collateral === 1,
    events: row.events,
  };
}

/**
 * Reads the folded positions out of ClickHouse.
 *
 * Two shapes worth explaining, because both are deliberate:
 *
 * **Filter and limit before joining the registry.** The page is at most
 * `limit + 1` rows by the time the join runs, so the join's cost cannot grow
 * with the table. It is a join at all — where the collateral flag is folded in
 * with a `UNION ALL` — because the registry is keyed without `user` and is
 * bounded by the number of reserves that will ever exist (fourteen today), not
 * by user activity.
 *
 * **Every wide integer is `toString`-ed in SQL.** Not left to the JSON encoder's
 * defaults: a share balance past 2^53 that arrives as a number has already lost
 * its tail by the time it reaches this process (§7.5).
 */
@Injectable()
export class ClickHousePositionStore implements PositionStore {
  constructor(@Inject(CLICKHOUSE_CLIENT) private readonly client: ClickHouseClient) {}

  async list(query: PositionQuery): Promise<PositionPage> {
    const params: Record<string, unknown> = {
      chainId: query.chainId,
      // Fetch one more than asked. Its presence is what says there is a next
      // page; counting the whole result set to find out would defeat the point
      // of keyset paging.
      limit: query.limit + 1,
    };
    const filters = [
      `chain_id = {chainId:UInt32}`,
      // Deliberately `!= 0` rather than §12.1's `> 0`. Shares cannot go negative
      // on chain, so a negative fold is drift — and it should surface as a
      // visibly wrong number for §9 to catch, not vanish behind the filter that
      // hides closed positions.
      `(supplied_shares != 0 OR drawn_shares != 0)`,
    ];

    if (query.user !== undefined) {
      filters.push(`user = {user:String}`);
      params['user'] = query.user.toLowerCase();
    }
    if (query.spoke !== undefined) {
      filters.push(`spoke = {spoke:String}`);
      params['spoke'] = query.spoke.toLowerCase();
    }
    if (query.cursor !== undefined) {
      const cursor = decodeCursor(query.cursor);
      // A lexicographic tuple comparison, and the tuple is the sorting key after
      // chain_id — so resuming seeks rather than scans.
      filters.push(
        `(user, spoke, reserve_id) > ({afterUser:String}, {afterSpoke:String}, {afterReserve:UInt256})`,
      );
      params['afterUser'] = cursor.user;
      params['afterSpoke'] = cursor.spoke;
      params['afterReserve'] = cursor.reserveId;
    }

    const result = await this.client.query({
      query: `
        SELECT
            p.chain_id                      AS chain_id,
            p.user                          AS user,
            p.spoke                         AS spoke,
            toString(p.reserve_id)          AS reserve_id,
            toString(r.asset_id)            AS asset_id,
            r.hub                           AS hub,
            toString(p.supplied_shares)     AS supplied_shares,
            toString(p.drawn_shares)        AS drawn_shares,
            toString(p.premium_shares)      AS premium_shares,
            toString(p.premium_offset_ray)  AS premium_offset_ray,
            toString(p.net_supplied_amount) AS net_supplied_amount,
            toString(p.net_borrowed_amount) AS net_borrowed_amount,
            p.using_as_collateral           AS using_as_collateral,
            toInt32(p.events)               AS events
        FROM (
            SELECT *
            FROM ${POSITIONS_VIEW}
            WHERE ${filters.join(' AND ')}
            ORDER BY user, spoke, reserve_id
            LIMIT {limit:UInt32}
        ) AS p
        LEFT JOIN ${RESERVES_VIEW} AS r USING (chain_id, spoke, reserve_id)
        -- Qualified, and it has to be. Unqualified, \`reserve_id\` binds to the
        -- toString alias above and sorts the decimal digits as text, putting 13
        -- before 3 — which then hands the next page a cursor from the wrong row.
        ORDER BY p.user, p.spoke, p.reserve_id`,
      query_params: params,
      format: 'JSONEachRow',
      // Without this a missing registry row reads as asset_id 0 and hub '',
      // which is indistinguishable from reserve 0 on a real hub.
      clickhouse_settings: { join_use_nulls: 1 },
    });

    const rows = await result.json<Row>();
    const items = rows.slice(0, query.limit).map(toPosition);
    const last = items.at(-1);

    return {
      items,
      nextCursor:
        rows.length > query.limit && last !== undefined
          ? encodeCursor({ user: last.user, spoke: last.spoke, reserveId: last.reserveId })
          : null,
    };
  }
}
