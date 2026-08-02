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

  describe('all — the sweep', () => {
    it('finds every underlying the Hub has listed', async () => {
      await events.append([
        addAsset({ block: 24_722_784, log: 0 }, USDC, 6, '1'),
        addAsset({ block: 24_722_784, log: 1 }, WETH, 18, '2'),
        updateAsset({ block: 24_730_000 }, undefined, undefined, undefined, '1'),
      ]);

      // The sweep is what makes bootstrap work: these fired far behind any
      // live cursor, so a range-scoped query would never see them.
      expect([...(await listings.all(CHAIN_ID))].toSorted()).toEqual([USDC, WETH].toSorted());
    });

    it('does not repeat an asset with a long checkpoint history', async () => {
      await events.append([
        addAsset({ block: 24_722_784 }, USDC, 6, '1'),
        ...Array.from({ length: 40 }, (_, index) =>
          updateAsset({ block: 24_730_000 + index }, undefined, undefined, undefined, '1'),
        ),
      ]);

      // `UpdateAsset` rows carry a null underlying, which is why discovery can
      // read the event-grain table directly instead of paying for the collapse
      // in `hub_assets_current`.
      expect(await listings.all(CHAIN_ID)).toEqual([USDC]);
    });

    it('finds nothing on a chain it has never indexed', async () => {
      await events.append([addAsset({ block: 24_722_784 }, USDC, 6, '1')]);

      expect(await listings.all(8453)).toEqual([]);
    });
  });

  describe('addedIn — the fast path', () => {
    it('finds an underlying listed inside the range', async () => {
      await events.append([addAsset({ block: 25_000_000 }, USDC, 6, '1')]);

      expect(await listings.addedIn(CHAIN_ID, 24_999_000, 25_001_000)).toEqual([USDC]);
    });

    it('finds nothing in a range that listed nothing', async () => {
      await events.append([
        addAsset({ block: 25_000_000 }, USDC, 6, '1'),
        updateAsset({ block: 25_500_000 }, undefined, undefined, undefined, '1'),
      ]);

      // Which is every range after genesis, and why the fast path has to be
      // cheap rather than merely correct.
      expect(await listings.addedIn(CHAIN_ID, 25_400_000, 25_600_000)).toEqual([]);
    });

    it('is inclusive at both ends, matching a dispatched range', async () => {
      await events.append([
        addAsset({ block: 25_000_000, log: 0 }, USDC, 6, '1'),
        addAsset({ block: 25_000_100, log: 0 }, WETH, 18, '2'),
      ]);

      const found = await listings.addedIn(CHAIN_ID, 25_000_000, 25_000_100);
      expect([...found].toSorted()).toEqual([USDC, WETH].toSorted());
    });

    it('still reports a listing the ledger has retracted', async () => {
      await events.append([addAsset({ block: 25_000_000 }, USDC, 6, '1')]);
      await events.revert(CHAIN_ID, 25_000_000, 25_000_000);

      // Deliberately lax, and pinned so nobody "fixes" it into a `sign > 0`
      // filter — which would look like it excluded reorged listings while the
      // original `sign = +1` row sat there until a merge. Over-fetching a
      // rolled-back token writes one row nothing joins to; missing one leaves
      // a position permanently unlabelled. Only the second is a defect.
      expect(await listings.addedIn(CHAIN_ID, 25_000_000, 25_000_000)).toEqual([USDC]);
    });
  });
});
