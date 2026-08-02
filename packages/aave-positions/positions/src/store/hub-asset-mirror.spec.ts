import type { ClickHouseClient } from '@clickhouse/client';
import { ClickHouseHubEventStore, type DecodedEvent } from '@aave-positions/events';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import { ClickHouseHubAssetStore } from './clickhouse-hub-asset-store';
import type { HubAsset } from './hub-asset';
import {
  HUB,
  HUB_TABLES,
  RAY,
  USDC,
  add,
  addAsset,
  draw,
  eliminateDeficit,
  mintFeeShares,
  reclaim,
  refreshPremium,
  remove,
  reportDeficit,
  restore,
  sweep,
  updateAsset,
  updateAssetConfig,
} from '../test-support/hub-ledger';
import { CHAIN_ID, migratedDatabase } from '../test-support/spoke-ledger';

/** Its own database: sibling suites share table names and would truncate this one. */
const DATABASE = 'spec_hub_mirror';

let client: ClickHouseClient;
let events: ClickHouseHubEventStore;
let store: ClickHouseHubAssetStore;

/** What the processor does with a dispatched range: cancel, then write. */
async function index(from: number, to: number, batch: DecodedEvent[]): Promise<void> {
  await events.revert(CHAIN_ID, from, to);
  await events.append(batch);
}

async function asset(assetId = '7'): Promise<HubAsset> {
  const found = await store.get(CHAIN_ID, HUB, assetId);
  if (!found) throw new Error(`no mirror row for asset ${assetId}`);
  return found;
}

describe('the Hub asset mirror', () => {
  beforeAll(async () => {
    client = await migratedDatabase(DATABASE);
    events = new ClickHouseHubEventStore(client);
    store = new ClickHouseHubAssetStore(client);
  });

  afterAll(async () => {
    await client.close();
  });

  beforeEach(async () => {
    for (const table of HUB_TABLES)
      // oxlint-disable-next-line no-await-in-loop
      await client.command({ query: `TRUNCATE TABLE ${table}` });
  });

  describe('the additive transitions', () => {
    it('adds supply and its liquidity', async () => {
      await index(100, 100, [add({ block: 100 }, '1000', '1200')]);

      expect(await asset()).toMatchObject({ addedShares: '1000', liquidity: '1200' });
    });

    it('takes both back on Remove', async () => {
      await index(100, 200, [
        add({ block: 100 }, '1000', '1200'),
        remove({ block: 200 }, '400', '500'),
      ]);

      expect(await asset()).toMatchObject({ addedShares: '600', liquidity: '700' });
    });

    it('moves liquidity out on Draw and debt shares in', async () => {
      await index(100, 200, [
        add({ block: 100 }, '1000', '1000'),
        draw({ block: 200 }, '300', '350'),
      ]);

      // Draw takes cash out of the Hub and records debt against it.
      expect(await asset()).toMatchObject({ drawnShares: '300', liquidity: '650' });
    });

    it('credits Restore with the premium as well as the principal', async () => {
      await index(100, 300, [
        add({ block: 100 }, '1000', '1000'),
        draw({ block: 200 }, '300', '300'),
        restore({ block: 300 }, '300', '300', '25'),
      ]);

      // §5.5's table says `liquidity += drawnAmount`. Hub.sol:291 computes
      // `asset.liquidity + drawnAmount + premiumAmount` — the premium is cash
      // arriving too, and a fold reading only `drawnAmount` loses it silently.
      expect(await asset()).toMatchObject({ drawnShares: '0', liquidity: '1025' });
    });

    it('writes debt off to deficit without returning cash', async () => {
      await index(100, 300, [
        add({ block: 100 }, '1000', '1000'),
        draw({ block: 200 }, '300', '300'),
        reportDeficit({ block: 300 }, '300', '2950000000000000000000000000000'),
      ]);

      // The shares leave and no liquidity comes back — the loss sits in
      // deficitRay, inside aggregatedOwedRay, until eliminated (§12.3).
      expect(await asset()).toMatchObject({
        drawnShares: '0',
        liquidity: '700',
        deficitRay: '2950000000000000000000000000000',
      });
    });

    it('burns the covering spoke shares when a deficit is eliminated', async () => {
      await index(100, 400, [
        add({ block: 100 }, '1000', '1000'),
        draw({ block: 200 }, '300', '300'),
        reportDeficit({ block: 300 }, '300', '2950000000000000000000000000000'),
        eliminateDeficit({ block: 400 }, '250', '2950000000000000000000000000000'),
      ]);

      // Not in the analysis's transition table at all — read from
      // Hub.eliminateDeficit:333-359. addedShares and deficitRay fall together,
      // which is what preserves the share price for everyone else.
      expect(await asset()).toMatchObject({ addedShares: '750', deficitRay: '0' });
    });

    it('mints fee shares as supply', async () => {
      await index(100, 200, [
        add({ block: 100 }, '1000', '1000'),
        mintFeeShares({ block: 200 }, '40', '41'),
      ]);

      expect(await asset()).toMatchObject({ addedShares: '1040' });
    });

    it('moves liquidity to swept and back', async () => {
      await index(100, 300, [
        add({ block: 100 }, '1000', '1000'),
        sweep({ block: 200 }, '400'),
        reclaim({ block: 300 }, '150'),
      ]);

      // Swept liquidity is still the asset's — totalAddedAssets counts both
      // sides, so the two have to move together or valuation drifts.
      expect(await asset()).toMatchObject({ liquidity: '750', swept: '250' });
    });

    it('accumulates a signed premium pair', async () => {
      await index(100, 200, [
        refreshPremium({ block: 100 }, '1000', '-500'),
        refreshPremium({ block: 200 }, '250', '-125'),
      ]);

      // Not in the analysis's table either. `_validateApplyPremiumDelta` adds
      // both deltas verbatim, which is what makes them additive rather than
      // latest-wins — and offsetRayDelta is int256 and really goes negative.
      expect(await asset()).toMatchObject({ premiumShares: '1250', premiumOffsetRay: '-625' });
    });
  });

  describe('the latest-wins transitions', () => {
    it('keeps the newest interest checkpoint, with the block it came from', async () => {
      await index(100, 200, [
        updateAsset({ block: 100 }, RAY, '50', '10'),
        updateAsset({ block: 200 }, '1000000000000000000000000009', '60', '20'),
      ]);

      const row = await asset();
      expect(row).toMatchObject({
        drawnIndex: '1000000000000000000000000009',
        drawnRate: '60',
        realizedFees: '20',
      });
      // §5.3: the checkpoint is (index, rate, t), and the event carries no t of
      // its own — the block's is it. Without this the index cannot be
      // extrapolated and every debt is stale by however long since the
      // checkpoint. Block 200's timestamp, not block 100's.
      expect(row.indexTimestamp).toBe('2026-07-25T17:23:20Z');
      expect(row.indexTimestamp).not.toBe('2026-07-25T17:21:40Z');
    });

    it('reports the zeroed fees a mint leaves behind', async () => {
      await index(100, 100, [
        mintFeeShares({ block: 100, log: 0 }, '40', '41'),
        updateAsset({ block: 100, log: 1 }, RAY, '0', '0'),
      ]);

      // `_mintFeeShares` sets realizedFees = 0 and does not say so in its own
      // event — but every function that calls accrue() also calls
      // updateDrawnRate(), all 14 of them, so an UpdateAsset carrying the zero
      // always follows in the same transaction at a higher log_index. That
      // ordering is the whole reason realizedFees can be latest-wins.
      expect(await asset()).toMatchObject({ realizedFees: '0', addedShares: '40' });
    });

    it('keeps the three latest-wins groups from overwriting each other', async () => {
      await index(100, 300, [
        addAsset({ block: 100 }, USDC, 6),
        updateAssetConfig({ block: 200 }, 1000),
        updateAsset({ block: 300 }, RAY, '50', '0'),
      ]);

      // The three fire at wildly different rates — 29,482 UpdateAsset against 34
      // configs and 17 listings over all history. One argMax over the whole row
      // would let the newest UpdateAsset null out `underlying` every twenty
      // seconds; each group guards on its own column instead.
      expect(await asset()).toMatchObject({
        underlying: USDC,
        decimals: 6,
        liquidityFee: 1000,
        drawnIndex: RAY,
      });
    });

    it('lower-cases the underlying, whatever the ABI hands back', async () => {
      await index(100, 100, [
        addAsset({ block: 100 }, '0xA0b86991C6218b36c1d19D4a2e9Eb0cE3606eB48', 6),
      ]);

      expect((await asset()).underlying).toBe(USDC);
    });
  });

  describe('reorgs and re-dispatch', () => {
    const seed = () =>
      index(100, 300, [
        add({ block: 100 }, '1000', '1000'),
        draw({ block: 200 }, '300', '300'),
        updateAsset({ block: 300 }, '1000000000000000000000000009', '60', '5'),
      ]);

    it('leaves the mirror unchanged when a range is dispatched twice', async () => {
      await seed();
      await seed();

      // Dispatch is at-least-once. Every value is multiplied by the source sign,
      // so the retraction of the first pass exactly negates it.
      expect(await asset()).toMatchObject({
        addedShares: '1000',
        drawnShares: '300',
        liquidity: '700',
        events: 2,
      });
    });

    it('returns the earlier checkpoint when the later one is retracted', async () => {
      await index(100, 100, [updateAsset({ block: 100 }, RAY, '50', '1')]);
      await index(200, 200, [
        updateAsset({ block: 200 }, '1000000000000000000000000009', '60', '2'),
      ]);

      // A reorg that removes block 200 and replaces it with nothing.
      await events.revert(CHAIN_ID, 200, 200);

      // The HAVING collapse is what makes this work: a retraction is a pair, and
      // the +1 twin still carries sign = 1, so filtering rows by sign would keep
      // returning the retracted checkpoint.
      expect(await asset()).toMatchObject({ drawnIndex: RAY, drawnRate: '50' });
    });

    it('takes the additive half back to exactly where it was', async () => {
      await seed();
      await index(400, 500, [add({ block: 400 }, '77', '88'), sweep({ block: 450 }, '11')]);

      await events.revert(CHAIN_ID, 400, 500);

      expect(await asset()).toMatchObject({
        addedShares: '1000',
        liquidity: '700',
        swept: '0',
        events: 2,
      });
    });

    it('reads the latest checkpoint by chain order, not dispatch order', async () => {
      // Block 200 indexed first, then block 100 re-dispatched — which the loop
      // does whenever a later processor asks to retry, and which gives block 100
      // the *higher* version.
      await index(200, 200, [
        updateAsset({ block: 200 }, '1000000000000000000000000009', '60', '0'),
      ]);
      await index(100, 100, [updateAsset({ block: 100 }, RAY, '50', '0')]);

      // Ordering the argMax by `version` returns block 100's stale checkpoint.
      expect(await asset()).toMatchObject({ drawnIndex: '1000000000000000000000000009' });
    });
  });

  describe('serialisation and scope', () => {
    it('returns a RAY-scaled index exact, as a string', async () => {
      await index(100, 100, [
        updateAsset({ block: 100 }, '1234567890123456789012345678901', '0', '0'),
      ]);

      const row = await asset();
      expect(row.drawnIndex).toBe('1234567890123456789012345678901');
      expect(typeof row.drawnIndex).toBe('string');
      expect(typeof row.assetId).toBe('string');
    });

    it('keeps two assets on one Hub apart', async () => {
      await index(100, 100, [
        add({ block: 100, log: 0 }, '1000', '1000', '7'),
        add({ block: 100, log: 1 }, '2000', '2000', '13'),
      ]);

      expect((await asset('7')).addedShares).toBe('1000');
      expect((await asset('13')).addedShares).toBe('2000');
    });

    it('keeps a listed asset whose balances net to zero', async () => {
      await index(100, 200, [
        addAsset({ block: 100 }, USDC, 6),
        add({ block: 150 }, '1000', '1000'),
        remove({ block: 200 }, '1000', '1000'),
      ]);

      // Unlike a position, an empty asset is still a real listing with a token
      // address and an index. Dropping it would leave a position against it
      // unvaluable rather than valued at zero.
      expect(await asset()).toMatchObject({ addedShares: '0', underlying: USDC, events: 2 });
    });
  });
});
