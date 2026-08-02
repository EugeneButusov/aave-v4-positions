import type { ClickHouseClient } from '@clickhouse/client';
import { ClickHouseSpokeEventStore, type DecodedEvent } from '@aave-positions/events';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import { ClickHousePositionStore } from './clickhouse-position-store';
import { PositionCursorCodec } from './position-cursor';
import {
  ALICE,
  BOB,
  CHAIN_ID,
  HUGE,
  ROUTER,
  SPOKE,
  TABLES,
  borrow,
  migratedDatabase,
  supply,
  withdraw,
} from '../test-support/spoke-ledger';

/** Its own database: sibling suites share table names and would truncate this one. */
const DATABASE = 'spec_position_store';

let client: ClickHouseClient;
let events: ClickHouseSpokeEventStore;
let store: ClickHousePositionStore;

/** What the processor does with a dispatched range: cancel, then write. */
async function index(from: number, to: number, batch: DecodedEvent[]): Promise<void> {
  await events.revert(CHAIN_ID, from, to);
  await events.append(batch);
}

const page = (over: Partial<Parameters<ClickHousePositionStore['list']>[0]> = {}) =>
  store.list({ chainId: CHAIN_ID, user: ALICE, spoke: SPOKE, limit: 100, ...over });

describe('ClickHousePositionStore', () => {
  beforeAll(async () => {
    client = await migratedDatabase(DATABASE);
    events = new ClickHouseSpokeEventStore(client);
    store = new ClickHousePositionStore(
      client,
      new PositionCursorCodec('spec-cursor-secret'.padEnd(32, '.')),
    );
  });

  afterAll(async () => {
    await client.close();
  });

  beforeEach(async () => {
    for (const table of TABLES)
      // oxlint-disable-next-line no-await-in-loop
      await client.command({ query: `TRUNCATE TABLE ${table}` });
  });

  describe('scope', () => {
    it('returns one wallet on one Spoke, and nobody else', async () => {
      await index(100, 100, [
        supply({ block: 100, log: 0 }, ALICE, '7', '500'),
        supply({ block: 100, log: 1 }, BOB, '7', '900'),
      ]);

      // `user` and `spoke` are required rather than optional filters: with
      // `chain_id` they are the whole sorting-key prefix above `reserve_id`, so
      // a page is a seek into contiguous rows rather than a scan.
      expect((await page()).items).toEqual([
        expect.objectContaining({ user: ALICE.toLowerCase(), suppliedShares: '500' }),
      ]);
    });

    it('lower-cases the address it is given', async () => {
      // The caller reads a checksummed address off a block explorer; the fold
      // stores it lower-cased. Neither side should have to know that.
      await index(100, 100, [supply({ block: 100 }, ALICE, '7', '500')]);

      expect((await page({ user: ALICE.toLowerCase() })).items).toHaveLength(1);
      expect((await page({ user: ALICE })).items).toHaveLength(1);
    });

    it('finds nothing on a Spoke the wallet has never touched', async () => {
      await index(100, 100, [supply({ block: 100 }, ALICE, '7', '500')]);

      expect((await page({ spoke: `0x${'11'.repeat(20)}` })).items).toEqual([]);
    });

    it('credits the user, never the caller that routed for them', async () => {
      await index(100, 100, [supply({ block: 100 }, ALICE, '7', '500', ROUTER)]);

      // §2: reading `caller` attributes large parts of the book to a handful of
      // position managers.
      expect((await page()).items).toHaveLength(1);
      expect((await page({ user: ROUTER })).items).toEqual([]);
    });
  });

  describe('what counts as a position', () => {
    it('hides a closed one but keeps its history', async () => {
      await index(100, 200, [
        supply({ block: 100 }, ALICE, '7', '500'),
        withdraw({ block: 200 }, ALICE, '7', '500'),
      ]);

      // §12.1: a position exists while its shares are non-zero. Its history does
      // not stop having happened, so the row and its event count stay.
      expect((await page()).items).toEqual([]);
      const rows = await client.query({
        query: `SELECT sum(events) AS n FROM user_positions`,
        format: 'JSONEachRow',
      });
      expect((await rows.json<{ n: number | string }>())[0]?.n).toEqual(2);
    });

    it('keeps a debt-only position, with no supply behind it', async () => {
      await index(100, 100, [borrow({ block: 100 }, ALICE, '13', '400')]);

      expect((await page()).items).toEqual([
        expect.objectContaining({ reserveId: '13', suppliedShares: '0', drawnShares: '400' }),
      ]);
    });
  });

  describe('serialisation', () => {
    it('returns every wide integer as a string, exact past 2^53', async () => {
      await index(100, 100, [supply({ block: 100 }, ALICE, '7', HUGE)]);

      const [position] = (await page()).items;
      // Not the JSON encoder's defaults: a share balance arriving as a number
      // has already lost its tail by the time it reaches this process (§7.5).
      expect(position?.suppliedShares).toBe(HUGE);
      expect(typeof position?.suppliedShares).toBe('string');
      expect(typeof position?.reserveId).toBe('string');
    });

    it('reports the collateral flag and the event count as their own types', async () => {
      await index(100, 100, [supply({ block: 100 }, ALICE, '7', '500')]);

      expect((await page()).items[0]).toMatchObject({
        chainId: CHAIN_ID,
        spoke: SPOKE,
        usingAsCollateral: false,
        events: 1,
      });
    });
  });

  describe('pagination', () => {
    const RESERVES = ['3', '7', '13', '21', '34'];

    const seed = () =>
      index(
        100,
        100,
        RESERVES.map((reserveId, i) => supply({ block: 100, log: i }, ALICE, reserveId, '500')),
      );

    it('walks every position exactly once, in numeric order', async () => {
      await seed();

      const seen: string[] = [];
      let cursor: string | undefined;
      do {
        // Sequential by definition — each page's cursor comes from the last one.
        // oxlint-disable-next-line no-await-in-loop
        const next = await page({ limit: 2, ...(cursor && { cursor }) });
        seen.push(...next.items.map((p) => p.reserveId));
        cursor = next.nextCursor ?? undefined;
      } while (cursor !== undefined);

      // Keyset, so the boundary is a key rather than a row count: nothing is
      // returned twice or skipped even though the indexer is free to write
      // between pages. Ordered as UInt256, so 13 precedes 21 rather than 3 —
      // which is what an unqualified ORDER BY over the toString alias got wrong.
      expect(seen).toEqual(RESERVES);
    });

    it('reports no next cursor when the page is not full', async () => {
      await seed();

      expect(await page({ limit: 5 })).toMatchObject({ nextCursor: null });
      expect(await page({ limit: 4 })).toMatchObject({ nextCursor: expect.any(String) });
    });

    it('resumes after the row the cursor names, not before it', async () => {
      await seed();

      const first = await page({ limit: 2 });
      const second = await page({ limit: 2, cursor: first.nextCursor ?? '' });

      expect(first.items.map((p) => p.reserveId)).toEqual(['3', '7']);
      expect(second.items.map((p) => p.reserveId)).toEqual(['13', '21']);
    });

    it("refuses one wallet's cursor replayed against another's listing", async () => {
      await index(100, 100, [
        supply({ block: 100, log: 0 }, ALICE, '7', '500'),
        supply({ block: 100, log: 1 }, ALICE, '13', '500'),
        supply({ block: 100, log: 2 }, BOB, '7', '500'),
        supply({ block: 100, log: 3 }, BOB, '13', '500'),
      ]);
      const alice = await page({ limit: 1 });

      // The signature covers the listing, not just the resume point. Unsigned,
      // this is a well-formed reserve id and Bob's first page would silently
      // start past it.
      await expect(page({ user: BOB, limit: 1, cursor: alice.nextCursor ?? '' })).rejects.toThrow(
        /does not match this listing/,
      );
    });

    it('refuses a cursor it never issued', async () => {
      await seed();

      await expect(page({ limit: 2, cursor: 'not-one-of-ours' })).rejects.toThrow(
        /invalid page cursor/,
      );
    });
  });
});
