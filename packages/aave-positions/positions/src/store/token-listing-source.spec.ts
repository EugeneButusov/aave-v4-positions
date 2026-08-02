import type { ClickHouseClient } from '@clickhouse/client';
import { ClickHouseHubEventStore } from '@aave-positions/events';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import { HUB_TABLES, addAsset, updateAsset } from '../test-support/hub-ledger';
import { CHAIN_ID, migratedDatabase } from '../test-support/spoke-ledger';
import { ClickHouseTokenListings } from './token-listing-source';

/** Its own database: sibling suites share table names and would truncate this one. */
const DATABASE = 'spec_token_listings';

const USDC = '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48';
const WETH = '0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2';

let client: ClickHouseClient;
let events: ClickHouseHubEventStore;
let listings: ClickHouseTokenListings;

describe('ClickHouseTokenListings', () => {
  beforeAll(async () => {
    client = await migratedDatabase(DATABASE);
    events = new ClickHouseHubEventStore(client);
    listings = new ClickHouseTokenListings(client);
  });

  afterAll(async () => {
    await client.close();
  });

  beforeEach(async () => {
    for (const table of HUB_TABLES)
      // oxlint-disable-next-line no-await-in-loop
      await client.command({ query: `TRUNCATE TABLE ${table}` });
  });

  it('finds every underlying the Hub has listed', async () => {
    await events.append([
      addAsset({ block: 24_722_784, log: 0 }, USDC, 6, '1'),
      addAsset({ block: 24_722_784, log: 1 }, WETH, 18, '2'),
    ]);

    expect([...(await listings.all(CHAIN_ID))].toSorted()).toEqual([USDC, WETH].toSorted());
  });

  it('reports one token however many checkpoints it has', async () => {
    await events.append([
      addAsset({ block: 24_722_784 }, USDC, 6, '1'),
      ...Array.from({ length: 40 }, (_, index) =>
        updateAsset({ block: 24_730_000 + index }, undefined, undefined, undefined, '1'),
      ),
    ]);

    // `UpdateAsset` writes a NULL `underlying` every time, so this is what the
    // `IS NOT NULL` filter is for — and, because the column is then sparse,
    // also why the query does not get slower as they pile up. Measured at
    // 3,029,631 rows: 34 rows read, unchanged from 29,631.
    expect(await listings.all(CHAIN_ID)).toEqual([USDC]);
  });

  it('answers from the projection rather than scanning the column', async () => {
    await events.append([
      addAsset({ block: 24_722_784 }, USDC, 6, '1'),
      ...Array.from({ length: 200 }, (_, index) =>
        updateAsset({ block: 24_730_000 + index }, undefined, undefined, undefined, '1'),
      ),
    ]);

    await listings.all(CHAIN_ID);
    await client.command({ query: 'SYSTEM FLUSH LOGS' });

    // `DISTINCT` over a column outside the sorting key is a full scan, and its
    // cost grows with `UpdateAsset` history that has no bearing on the answer.
    // The `listed_tokens` projection on `hub_asset_state` is what stops that,
    // and it is chosen by the optimizer — so nothing in the query says it is
    // being used, and nothing would say if it stopped.
    const used = await client.query({
      query: `
        SELECT arrayStringConcat(projections, ',') AS used
        FROM system.query_log
        WHERE type = 'QueryFinish'
          AND current_database = {database:String}
          AND query LIKE 'SELECT DISTINCT underlying%'
        ORDER BY event_time DESC
        LIMIT 1
      `,
      query_params: { database: DATABASE },
      format: 'JSONEachRow',
    });

    const [row] = await used.json<{ used: string }>();
    expect(row?.used).toContain('listed_tokens');
  });

  it('finds nothing on a chain it has never indexed', async () => {
    await events.append([addAsset({ block: 24_722_784 }, USDC, 6, '1')]);

    expect(await listings.all(8453)).toEqual([]);
  });

  it('still reports a listing the ledger has retracted', async () => {
    await events.append([addAsset({ block: 25_000_000 }, USDC, 6, '1')]);
    await events.revert(CHAIN_ID, 25_000_000, 25_000_000);

    // Deliberately lax, and pinned so nobody "fixes" it into a `sign > 0`
    // filter — which would look like it excluded reorged listings while the
    // original `sign = +1` row sat there until a merge. Over-fetching a
    // rolled-back token costs one wasted ERC-20 read; missing one leaves a
    // position permanently unlabelled. Only the second is a defect.
    expect(await listings.all(CHAIN_ID)).toEqual([USDC]);
  });

  it('reports a token listed twice only once', async () => {
    // Nothing rules out two asset ids sharing an underlying, and the caller
    // diffs this against a keyed table — a duplicate would read as a gap that
    // never closes.
    await events.append([
      addAsset({ block: 24_722_784, log: 0 }, USDC, 6, '1'),
      addAsset({ block: 24_722_784, log: 1 }, USDC, 6, '2'),
    ]);

    expect(await listings.all(CHAIN_ID)).toEqual([USDC]);
  });
});
