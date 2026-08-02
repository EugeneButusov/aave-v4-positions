import { BadRequestException, NotFoundException } from '@nestjs/common';
import {
  type Position,
  type PositionPage,
  type PositionQuery,
  type PositionStore,
} from '@aave-positions/positions';
import type { Hash, SyncStatus, SyncStatusStore } from '@packages/indexing';
import { beforeEach, describe, expect, it } from 'vitest';

import { PositionCursors } from './position-cursors';
import { PositionsService } from './positions.service';

const CHAIN_ID = 1;
const ALICE = '0x82d16ff1c724ab72f218a3f7f6dd3e5385ee87e8';
const SPOKE = '0x94e7a5dcbe816e498b89ab752661904e2f56c485';
const HASH: Hash = `0x${'ab'.repeat(32)}`;
const HUGE = '422166581625087607993';
const HUB = '0xcca852bc40e560adc3b1cc58ca5b55638ce826c9';
const USDC = '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48';
const RAY = '1000000000000000000000000000';
const VALUED_AT = 1_785_000_000;

const STALE_AFTER = 60;

const cursors = new PositionCursors('spec-cursor-secret'.padEnd(32, '.'));

function position(over: Partial<Position> = {}): Position {
  return {
    chainId: CHAIN_ID,
    user: ALICE,
    spoke: SPOKE,
    reserveId: '7',
    suppliedShares: HUGE,
    drawnShares: '0',
    premiumShares: '0',
    premiumOffsetRay: '0',
    netSuppliedAmount: HUGE,
    netBorrowedAmount: '0',
    usingAsCollateral: true,
    events: 3,
    asset: { assetId: '7', hub: HUB, underlying: USDC, decimals: 6 },
    value: {
      suppliedAmount: '1000000000',
      drawnDebt: '0',
      premiumDebt: '0',
      totalDebt: '0',
      drawnIndex: RAY,
    },
    ...over,
  };
}

/** Records the query it was handed, which is most of what this service decides. */
class RecordingStore implements PositionStore {
  readonly queries: PositionQuery[] = [];
  page: PositionPage = { items: [], valuedAt: VALUED_AT, next: null };

  list(query: PositionQuery): Promise<PositionPage> {
    this.queries.push(query);
    return Promise.resolve(this.page);
  }
}

class FixedSync implements SyncStatusStore {
  constructor(private readonly status: SyncStatus | null) {}

  get(): Promise<SyncStatus | null> {
    return Promise.resolve(this.status);
  }
}

function syncAt(ageSeconds: number): SyncStatus {
  return {
    chainId: CHAIN_ID,
    lastBlock: 25_652_535,
    lastHash: HASH,
    updatedAt: new Date('2026-08-02T11:04:31.221Z'),
    ageSeconds,
  };
}

function serviceWith(store: PositionStore, sync: SyncStatusStore): PositionsService {
  return new PositionsService(store, sync, STALE_AFTER, cursors);
}

describe('PositionsService', () => {
  let store: RecordingStore;
  let service: PositionsService;

  beforeEach(() => {
    store = new RecordingStore();
    service = serviceWith(store, new FixedSync(syncAt(7)));
  });

  const list = (query: Parameters<PositionsService['list']>[1] = { limit: 50 }) =>
    service.list({ chainId: CHAIN_ID, user: ALICE }, query);

  describe('the query it builds', () => {
    it('omits the Spoke entirely when none was asked for', async () => {
      await list({ limit: 50 });

      // Absent, not `undefined`-valued: the store branches on `!== undefined`,
      // and a present-but-undefined key reads the same to it but not to anyone
      // logging or asserting on the object.
      expect(store.queries[0]).toEqual({ chainId: CHAIN_ID, user: ALICE, limit: 50 });
      expect(store.queries[0]).not.toHaveProperty('spoke');
    });

    it('passes the Spoke and the limit through when they are given', async () => {
      await list({ limit: 5, spoke: SPOKE });

      expect(store.queries[0]).toMatchObject({ spoke: SPOKE, limit: 5 });
    });

    it('starts with no resume point when there is no cursor', async () => {
      await list({ limit: 50 });

      expect(store.queries[0]).not.toHaveProperty('after');
    });
  });

  describe('cursors', () => {
    it('round-trips a page key through the wire format', async () => {
      store.page = {
        valuedAt: VALUED_AT,
        items: [position()],
        next: { spoke: SPOKE, reserveId: '7' },
      };

      const first = await list({ limit: 1 });
      expect(first.nextCursor).toEqual(expect.any(String));

      await list({ limit: 1, cursor: first.nextCursor ?? '' });

      // What the caller held was opaque; what the store receives is the key.
      expect(store.queries[1]?.after).toEqual({ spoke: SPOKE, reserveId: '7' });
    });

    it('reports the end of the listing as null rather than an empty token', async () => {
      store.page = { valuedAt: VALUED_AT, items: [position()], next: null };

      await expect(list()).resolves.toMatchObject({ nextCursor: null });
    });

    it('refuses a cursor issued for a different Spoke filter', async () => {
      store.page = {
        valuedAt: VALUED_AT,
        items: [position()],
        next: { spoke: SPOKE, reserveId: '7' },
      };
      const broad = await list({ limit: 1 });

      // An all-Spokes resume point is well-formed inside a narrowed listing, so
      // unsigned it would silently skip every reserve below it.
      await expect(
        list({ limit: 1, spoke: SPOKE, cursor: broad.nextCursor ?? '' }),
      ).rejects.toThrow(BadRequestException);
    });

    it('answers 400 for a cursor it did not issue, not 500', async () => {
      // A caller's bad input is not the service failing. A 500 here would be a
      // page an operator gets woken for.
      await expect(list({ limit: 50, cursor: 'not-one-of-ours' })).rejects.toThrow(
        BadRequestException,
      );
    });
  });

  describe('sync metadata', () => {
    it('stamps every page with the block it is true as of', async () => {
      await expect(list()).resolves.toMatchObject({
        sync: {
          lastBlock: 25_652_535,
          lastBlockHash: HASH,
          updatedAt: '2026-08-02T11:04:31.221Z',
          ageSeconds: 7,
          stale: false,
        },
      });
    });

    it.each([
      ['below the threshold', STALE_AFTER - 1, false],
      ['exactly at it', STALE_AFTER, false],
      ['past it', STALE_AFTER + 1, true],
    ])('reports an age %s as stale=%s', async (_case, ageSeconds, stale) => {
      service = serviceWith(store, new FixedSync(syncAt(ageSeconds)));

      await expect(list()).resolves.toMatchObject({ sync: { stale } });
    });

    it('answers 404 for a chain this deployment has never indexed', async () => {
      service = serviceWith(store, new FixedSync(null));

      // Not an empty 200. That would report "this wallet holds nothing" for
      // what is really a request aimed at the wrong deployment.
      await expect(list()).rejects.toThrow(NotFoundException);
      expect(store.queries).toEqual([]);
    });
  });

  it('maps the domain type onto the wire contract field by field', async () => {
    store.page = { valuedAt: VALUED_AT, items: [position({ drawnShares: '400' })], next: null };

    const { items } = await list();

    // Not the domain object passed through: `Position` gains fields when Hub
    // ingestion lands, and the published contract must not move with it.
    expect(items).toEqual([
      {
        chainId: CHAIN_ID,
        user: ALICE,
        spoke: SPOKE,
        reserveId: '7',
        suppliedShares: HUGE,
        drawnShares: '400',
        premiumShares: '0',
        premiumOffsetRay: '0',
        netSuppliedAmount: HUGE,
        netBorrowedAmount: '0',
        usingAsCollateral: true,
        events: 3,
        asset: { assetId: '7', hub: HUB, underlying: USDC, decimals: 6 },
        value: {
          suppliedAmount: '1000000000',
          drawnDebt: '0',
          premiumDebt: '0',
          totalDebt: '0',
          drawnIndex: RAY,
        },
      },
    ]);
  });
});
