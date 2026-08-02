import type { INestApplication } from '@nestjs/common';
import { Test } from '@nestjs/testing';
import { RESERVE_PRICE_STORE, TOKEN_METADATA_STORE, reserveKey } from '@aave-positions/enrichment';
import {
  POSITION_STORE,
  type Position,
  type PositionHoldings,
  type PositionPage,
  type PositionQuery,
  type PositionStore,
} from '@aave-positions/positions';
import {
  SYNC_STATUS_STORE,
  type Hash,
  type SyncStatus,
  type SyncStatusStore,
} from '@packages/indexing';
import request from 'supertest';
import type { App } from 'supertest/types';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import { AppModule } from '../src/app.module';
import { httpSetup } from '../src/http.setup';

const CHAIN_ID = 1;
const ALICE = '0x82d16ff1c724ab72f218a3f7f6dd3e5385ee87e8';
const SPOKE = '0x94e7a5dcbe816e498b89ab752661904e2f56c485';
const SECOND_SPOKE = '0x973a023a77420ba610f06b3858ad991df6d85a08';
const HASH: Hash = `0x${'ab'.repeat(32)}`;
/** Past 2^53, where a JSON number would silently lose the tail. */
const HUGE = '422166581625087607993';
const HUB = '0xcca852bc40e560adc3b1cc58ca5b55638ce826c9';
const USDC = '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48';
const RAY = '1000000000000000000000000000';
const VALUED_AT = 1_785_000_000;

const PATH = `/api/v1/chains/${CHAIN_ID}/users/${ALICE}/positions`;

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

/**
 * The same position as it goes out on the wire.
 *
 * Separate from {@link position} rather than reused, because the two shapes
 * have stopped matching: the domain type knows nothing about labels, and the
 * response carries them. Asserting the response against the domain fixture
 * would quietly stop checking the mapper the moment the wire gained a field.
 */
function wirePosition(
  label: { symbol: string | null; name: string | null } | null = null,
  usd: { price: string; suppliedValue: string; debtValue: string } | null = null,
): unknown {
  const { asset, value, ...rest } = position();
  return {
    ...rest,
    asset: { ...asset, symbol: label?.symbol ?? null, name: label?.name ?? null },
    value: {
      ...value,
      price: usd?.price ?? null,
      suppliedValue: usd?.suppliedValue ?? null,
      debtValue: usd?.debtValue ?? null,
    },
  };
}

/**
 * Stands in for ClickHouse. What the real store does is pinned where it lives,
 * against a real server; what is unproven here is the HTTP layer, and a database
 * proves none of it.
 */
class FakeStore implements PositionStore {
  readonly queries: PositionQuery[] = [];
  page: PositionPage = { items: [], valuedAt: VALUED_AT, next: null };

  list(query: PositionQuery): Promise<PositionPage> {
    this.queries.push(query);
    return Promise.resolve(this.page);
  }

  /** The same rows unpaged, which is what a wallet fitting in one page holds. */
  holdings(): Promise<PositionHoldings> {
    return Promise.resolve({ items: this.page.items, valuedAt: VALUED_AT });
  }
}

class FakeSync implements SyncStatusStore {
  status: SyncStatus | null = {
    chainId: CHAIN_ID,
    lastBlock: 25_652_535,
    lastHash: HASH,
    updatedAt: new Date('2026-08-02T11:04:31.221Z'),
    ageSeconds: 7,
  };

  get(chainId: number): Promise<SyncStatus | null> {
    return Promise.resolve(this.status?.chainId === chainId ? this.status : null);
  }
}

/** Stands in for Postgres, so the wiring test needs no second database. */
class FakeTokens {
  labels = new Map<string, { symbol: string | null; name: string | null }>();

  put(): Promise<void> {
    return Promise.resolve();
  }
}

/** The other Postgres-backed enrichment, stubbed on the same terms. */
class FakePrices {
  prices = new Map<string, { price: string; pricedAt: Date; ageSeconds: number }>();

  put(): Promise<void> {
    return Promise.resolve();
  }
}

describe('positions (e2e)', () => {
  let app: INestApplication<App>;
  const store = new FakeStore();
  const sync = new FakeSync();
  const tokens = new FakeTokens();
  const prices = new FakePrices();

  beforeAll(async () => {
    const moduleRef = await Test.createTestingModule({ imports: [AppModule] })
      .overrideProvider(POSITION_STORE)
      .useValue(store)
      .overrideProvider(SYNC_STATUS_STORE)
      .useValue(sync)
      .overrideProvider(TOKEN_METADATA_STORE)
      .useValue({ labels: () => Promise.resolve(tokens.labels), put: () => Promise.resolve() })
      .overrideProvider(RESERVE_PRICE_STORE)
      .useValue({ latest: () => Promise.resolve(prices.prices), put: () => Promise.resolve() })
      .compile();

    app = moduleRef.createNestApplication({ logger: false });
    httpSetup(app, { globalPrefix: 'api' });
    await app.init();
  });

  afterAll(async () => {
    await app.close();
  });

  beforeEach(() => {
    store.queries.length = 0;
    store.page = { valuedAt: VALUED_AT, items: [], next: null };
  });

  describe('the happy path', () => {
    it('serves a page, stamped with the block it is true as of', async () => {
      store.page = { valuedAt: VALUED_AT, items: [position()], next: null };

      const res = await request(app.getHttpServer()).get(PATH).expect(200);

      expect(res.body).toEqual({
        sync: {
          lastBlock: 25_652_535,
          lastBlockHash: HASH,
          updatedAt: '2026-08-02T11:04:31.221Z',
          ageSeconds: 7,
          stale: false,
        },
        valuedAt: VALUED_AT,
        // Null with nothing priced, and it is a distinct field rather than an
        // absent one: a caller has to be able to tell "no prices behind this"
        // from "this deployment does not price".
        pricing: null,
        // The Spoke is here with nulls rather than absent: the wallet does hold
        // something on it, and only the value is unknown.
        portfolio: [{ spoke: SPOKE, suppliedValue: null, debtValue: null, netWorth: null }],
        items: [wirePosition()],
        nextCursor: null,
      });
    });

    it('serialises every wide integer as a string', async () => {
      store.page = { valuedAt: VALUED_AT, items: [position()], next: null };

      const res = await request(app.getHttpServer()).get(PATH).expect(200);

      // float64 has 53 bits of mantissa and share balances run far past it. The
      // failure mode is a few wei of drift, which reads as a rounding bug rather
      // than the parse error it is.
      expect(res.body.items[0].suppliedShares).toBe(HUGE);
      expect(typeof res.body.items[0].suppliedShares).toBe('string');
      expect(typeof res.body.items[0].reserveId).toBe('string');
      // The valued half too — it is the one a caller actually reads, and the
      // one where a wei of drift looks like a rounding bug rather than a bug.
      expect(typeof res.body.items[0].value.suppliedAmount).toBe('string');
      expect(typeof res.body.items[0].value.drawnIndex).toBe('string');
    });

    it('carries the token the reserve resolves to', async () => {
      store.page = { valuedAt: VALUED_AT, items: [position()], next: null };

      const res = await request(app.getHttpServer()).get(PATH).expect(200);

      // `reserveId` is a per-Spoke index and means nothing on its own. This is
      // the whole reason Hub ingestion exists, so it must survive the boundary
      // rather than being dropped by a mapper that predates it.
      expect(res.body.items[0].asset).toEqual({
        assetId: '7',
        hub: HUB,
        underlying: USDC,
        decimals: 6,
        symbol: null,
        name: null,
      });
    });

    it('labels the asset once enrichment has read the token', async () => {
      store.page = { valuedAt: VALUED_AT, items: [position()], next: null };
      tokens.labels.set(USDC, { symbol: 'USDC', name: 'USD Coin' });

      const res = await request(app.getHttpServer()).get(PATH).expect(200);

      // Two databases behind one response, which is the shape every later
      // enrichment lands in — the label comes from Postgres, everything around
      // it from ClickHouse, and the caller cannot tell.
      expect(res.body.items[0].asset).toMatchObject({ symbol: 'USDC', name: 'USD Coin' });
      tokens.labels.clear();
    });

    it('values the position once the oracle has been read', async () => {
      store.page = { valuedAt: VALUED_AT, items: [position()], next: null };
      prices.prices.set(reserveKey(SPOKE, '7'), {
        price: '99971505',
        pricedAt: new Date('2026-08-02T11:04:17.000Z'),
        ageSeconds: 41,
      });

      const res = await request(app.getHttpServer()).get(PATH).expect(200);

      // Three reads behind one response now — the fold from ClickHouse, the
      // label and the price from Postgres — and the caller cannot tell.
      expect(res.body.items[0].value).toMatchObject({
        price: '99971505',
        suppliedValue: '99971505000000000000000000000',
      });
      // The third clock, beside `sync` and `valuedAt`.
      expect(res.body.pricing).toEqual({
        updatedAt: '2026-08-02T11:04:17.000Z',
        ageSeconds: 41,
        stale: false,
      });
      // And the totals over the same rows, keyed by Spoke.
      expect(res.body.portfolio).toEqual([
        {
          spoke: SPOKE,
          suppliedValue: '99971505000000000000000000000',
          debtValue: '0',
          netWorth: '99971505000000000000000000000',
        },
      ]);
      prices.prices.clear();
    });

    it('withholds every price when asOf is set', async () => {
      store.page = { valuedAt: VALUED_AT, items: [position()], next: null };
      prices.prices.set(reserveKey(SPOKE, '7'), {
        price: '99971505',
        pricedAt: new Date('2026-08-02T11:04:17.000Z'),
        ageSeconds: 41,
      });

      const res = await request(app.getHttpServer())
        .get(`${PATH}?asOf=${VALUED_AT - 3600}`)
        .expect(200);

      // Amounts are extrapolated to that instant; the stored price is current.
      // A value mixing the two is a number that never existed.
      expect(res.body.pricing).toBeNull();
      expect(res.body.items[0].value).toMatchObject({ price: null, suppliedValue: null });
      prices.prices.clear();
    });

    it('reports null rather than zero when the join has nothing to offer', async () => {
      store.page = {
        valuedAt: VALUED_AT,
        items: [position({ asset: null, value: null })],
        next: null,
      };

      const res = await request(app.getHttpServer()).get(PATH).expect(200);

      // A zero amount here is indistinguishable from a real zero balance, and
      // JSON has a null for exactly this. The position still appears, because
      // its shares are real.
      expect(res.body.items[0]).toMatchObject({ asset: null, value: null, suppliedShares: HUGE });
    });

    it('values the page at the instant it was asked for, and says so', async () => {
      await request(app.getHttpServer()).get(`${PATH}?asOf=1785000123`).expect(200);

      // Amounts accrue every second with nothing emitted, so "now" is a choice.
      // Naming it is what makes two identical requests return identical numbers.
      expect(store.queries[0]?.asOf).toBe(1_785_000_123n);
    });

    it('answers an unknown wallet with an empty list, not a 404', async () => {
      // A wallet holding nothing is not a missing resource. The 404 is reserved
      // for a chain this deployment does not follow, which is a different fault.
      const res = await request(app.getHttpServer()).get(PATH).expect(200);

      expect(res.body.items).toEqual([]);
      expect(res.body.nextCursor).toBeNull();
    });

    it('takes a checksummed wallet and answers for the lower-cased one', async () => {
      const checksummed = '0x82D16fF1C724ab72F218A3f7f6DD3E5385ee87E8';

      await request(app.getHttpServer())
        .get(`/api/v1/chains/${CHAIN_ID}/users/${checksummed}/positions`)
        .expect(200);

      expect(store.queries[0]?.user).toBe(ALICE);
    });

    it('applies the default page size, and the one it is given', async () => {
      await request(app.getHttpServer()).get(PATH).expect(200);
      await request(app.getHttpServer()).get(`${PATH}?limit=5`).expect(200);

      expect(store.queries.map((q) => q.limit)).toEqual([50, 5]);
    });
  });

  describe('the Spoke filter', () => {
    it('lists every Spoke when none is named', async () => {
      await request(app.getHttpServer()).get(PATH).expect(200);

      expect(store.queries[0]).not.toHaveProperty('spoke');
    });

    it('narrows to one when it is named', async () => {
      await request(app.getHttpServer()).get(`${PATH}?spoke=${SPOKE}`).expect(200);

      expect(store.queries[0]?.spoke).toBe(SPOKE);
    });
  });

  describe('paging', () => {
    it('hands back a cursor that resumes where the page stopped', async () => {
      store.page = {
        valuedAt: VALUED_AT,
        items: [position()],
        next: { spoke: SPOKE, reserveId: '7' },
      };
      const first = await request(app.getHttpServer()).get(`${PATH}?limit=1`).expect(200);

      expect(first.body.nextCursor).toEqual(expect.any(String));

      store.page = { valuedAt: VALUED_AT, items: [position({ spoke: SECOND_SPOKE })], next: null };
      await request(app.getHttpServer())
        .get(`${PATH}?limit=1&cursor=${encodeURIComponent(first.body.nextCursor)}`)
        .expect(200);

      // The caller held an opaque token; the store is handed the key it names.
      expect(store.queries[1]?.after).toEqual({ spoke: SPOKE, reserveId: '7' });
    });

    it('issues a cursor that needs no escaping in a query string', async () => {
      store.page = {
        valuedAt: VALUED_AT,
        items: [position()],
        next: { spoke: SPOKE, reserveId: '7' },
      };

      const res = await request(app.getHttpServer()).get(`${PATH}?limit=1`).expect(200);

      expect(res.body.nextCursor).toMatch(/^[\w-]+\.[\w-]+$/);
    });

    it('refuses a cursor carried to a different Spoke filter', async () => {
      store.page = {
        valuedAt: VALUED_AT,
        items: [position()],
        next: { spoke: SPOKE, reserveId: '7' },
      };
      const broad = await request(app.getHttpServer()).get(`${PATH}?limit=1`).expect(200);

      // An all-Spokes resume point is well-formed inside a narrowed listing, so
      // without the scope in the signature this would silently skip every
      // reserve below it rather than failing.
      await request(app.getHttpServer())
        .get(`${PATH}?limit=1&spoke=${SPOKE}&cursor=${encodeURIComponent(broad.body.nextCursor)}`)
        .expect(400);
    });
  });

  describe('refusals', () => {
    it.each([
      ['a malformed wallet', `/api/v1/chains/1/users/0xnope/positions`],
      ['a non-numeric chain id', `/api/v1/chains/abc/users/${ALICE}/positions`],
      ['a malformed Spoke', `${PATH}?spoke=0xnope`],
      ['a page size of zero', `${PATH}?limit=0`],
      ['a page size past the cap', `${PATH}?limit=201`],
      ['a misspelled parameter', `${PATH}?limt=5`],
      ['an empty cursor', `${PATH}?cursor=`],
      ['an asOf in milliseconds', `${PATH}?asOf=1785000000000`],
      ['an asOf before the Spoke existed', `${PATH}?asOf=1`],
      ['a cursor it never issued', `${PATH}?cursor=not-one-of-ours`],
    ])('answers 400 for %s', async (_case, path) => {
      const res = await request(app.getHttpServer()).get(path).expect(400);

      expect(res.body.message).toEqual(expect.any(String));
      expect(store.queries).toEqual([]);
    });

    it('answers 404 for a chain this deployment has never indexed', async () => {
      // Not an empty 200. That would report "this wallet holds nothing" for a
      // request aimed at a chain the indexer does not follow.
      const res = await request(app.getHttpServer())
        .get(`/api/v1/chains/137/users/${ALICE}/positions`)
        .expect(404);

      expect(res.body.message).toMatch(/137/);
      expect(store.queries).toEqual([]);
    });
  });

  describe('how it is mounted', () => {
    it('is not served unversioned', async () => {
      await request(app.getHttpServer())
        .get(`/api/chains/${CHAIN_ID}/users/${ALICE}/positions`)
        .expect(404);
    });

    it('is not served unprefixed', async () => {
      await request(app.getHttpServer())
        .get(`/v1/chains/${CHAIN_ID}/users/${ALICE}/positions`)
        .expect(404);
    });
  });
});
