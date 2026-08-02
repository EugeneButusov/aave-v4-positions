import { join } from 'node:path';

import type { ClickHouseClient } from '@clickhouse/client';
import { Inject, Injectable } from '@nestjs/common';
import { CLICKHOUSE_CLIENT } from '@packages/clickhouse';

import type { Position, PositionAsset, PositionValue } from './position';
import { valuePosition, type AssetState } from '../valuation/valuation';
import type {
  PositionHoldings,
  PositionHoldingsQuery,
  PositionPage,
  PositionQuery,
  PositionStore,
} from './position-store';

const POSITIONS_VIEW = 'user_positions_current';
const RESERVES_VIEW = 'spoke_reserves_current';
const HUB_ASSETS_VIEW = 'hub_assets_current';

/**
 * The most positions one wallet is assumed to hold, across every Spoke.
 *
 * Not a page size — {@link ClickHousePositionStore.holdings} refuses above this
 * rather than truncating, because it exists to be summed and a short read is a
 * net worth that is quietly too small. Set far above what the protocol permits:
 * `maxUserReservesLimit` caps a wallet's reserves per Spoke, the Main Spoke
 * lists fourteen, and the address book knows fifteen Spokes.
 */
const HOLDINGS_CAP = 2_000;

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
 * Three shapes worth explaining, because all three are deliberate:
 *
 * **`chain_id` and `user` are required and bound**, which is the leading pair of
 * the sorting key. That is what makes a page a seek into one wallet's contiguous
 * rows rather than a filter over everyone's. `spoke` narrows further when it is
 * given, and its absence costs nothing: the prefix that does the seeking is
 * already pinned without it.
 *
 * **The resume point is the whole of what the prefix leaves free** — `(spoke,
 * reserve_id)`, compared as a pair even when `spoke` is pinned and the first
 * half is therefore constant. One comparison rather than two branches: a
 * `reserve_id`-only special case would silently read a resume point from one
 * Spoke against another's rows if the two ever disagreed.
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
      user: query.user.toLowerCase(),
      // Fetch one more than asked. Its presence is what says there is a next
      // page; counting the whole result set to find out would defeat the point
      // of keyset paging.
      limit: query.limit + 1,
    };
    const filters = [
      // The leading pair of the sorting key, so the scan starts at this wallet's
      // rows rather than filtering its way to them.
      `chain_id = {chainId:UInt32}`,
      `user = {user:String}`,
      // Deliberately `!= 0` rather than §12.1's `> 0`. Shares cannot go negative
      // on chain, so a negative fold is drift — and it should surface as a
      // visibly wrong number for §9 to catch, not vanish behind the filter that
      // hides closed positions.
      `(supplied_shares != 0 OR drawn_shares != 0)`,
    ];

    if (query.spoke !== undefined) {
      filters.push(`spoke = {spoke:String}`);
      params['spoke'] = query.spoke.toLowerCase();
    }

    if (query.after !== undefined) {
      filters.push(`(spoke, reserve_id) > ({afterSpoke:String}, {afterReserve:UInt256})`);
      params['afterSpoke'] = query.after.spoke;
      params['afterReserve'] = query.after.reserveId;
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
        -- before 3 — which then hands the next page a key from the wrong row.
        ORDER BY p.spoke, p.reserve_id`,
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
      next:
        rows.length > query.limit && last !== undefined
          ? { spoke: last.spoke, reserveId: last.reserveId }
          : null,
    };
  }

  /**
   * Every open position, in one read.
   *
   * **Built on {@link list} rather than on a second query**, and that is what
   * makes the bound safe rather than merely large. `list` already fetches one
   * more row than asked and reports the surplus as `next`, so "there were more
   * than {@link HOLDINGS_CAP}" is a question the paging machinery already
   * answers — and answering it is the whole reason this refuses instead of
   * truncating. A short read here is not a short page; it is a net worth that
   * is quietly too small, with nothing in the response to say so.
   *
   * Duplicating the SQL to drop one `LIMIT` would also duplicate the join
   * shape, the pushdown argument and the `ORDER BY` trap documented above, and
   * the copy that drifted would be the one nobody was reading.
   */
  async holdings(query: PositionHoldingsQuery): Promise<PositionHoldings> {
    const page = await this.list({
      chainId: query.chainId,
      user: query.user,
      ...(query.spoke !== undefined && { spoke: query.spoke }),
      ...(query.asOf !== undefined && { asOf: query.asOf }),
      limit: HOLDINGS_CAP,
    });

    if (page.next !== null) {
      throw new Error(
        `wallet ${query.user} holds more than ${String(HOLDINGS_CAP)} open positions on chain ` +
          `${String(query.chainId)}; refusing to total a partial set`,
      );
    }

    return { items: page.items, valuedAt: page.valuedAt };
  }
}
