import { BadRequestException, NotFoundException } from '@nestjs/common';
import {
  reserveKey,
  type Position,
  type PositionHoldings,
  type PositionHoldingsQuery,
  type PositionPage,
  type PositionQuery,
  type PositionStore,
  type ReservePrice,
  type ReservePriceStore,
  type TokenLabel,
  type TokenMetadataStore,
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
const PRICE_STALE_AFTER = 300;

/** $0.99971505, which is what the USDC feed actually reads (§7.4.3). */
const USDC_PRICE = '99971505';
const PRICED_AT = new Date('2026-08-02T11:04:17.000Z');

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
  readonly holdingQueries: PositionHoldingsQuery[] = [];
  page: PositionPage = { items: [], valuedAt: VALUED_AT, next: null };
  /**
   * What the unpaged read returns, when it differs from the page.
   *
   * Null means "the same rows", which is the case for a wallet that fits in one
   * page — and setting it is how a test says the totals cover more than the
   * caller can see.
   */
  beyondThePage: readonly Position[] | null = null;

  list(query: PositionQuery): Promise<PositionPage> {
    this.queries.push(query);
    return Promise.resolve(this.page);
  }

  holdings(query: PositionHoldingsQuery): Promise<PositionHoldings> {
    this.holdingQueries.push(query);
    return Promise.resolve({
      items: this.beyondThePage ?? this.page.items,
      valuedAt: VALUED_AT,
    });
  }
}

/** Records when it was asked, so the parallel read can be asserted rather than assumed. */
class RecordingTokens implements TokenMetadataStore {
  labelsByToken = new Map<string, TokenLabel>();
  /** Incremented synchronously, so "was it asked yet" is answerable. */
  calls = 0;

  labels(): Promise<ReadonlyMap<string, TokenLabel>> {
    this.calls += 1;
    return Promise.resolve(this.labelsByToken);
  }

  put(): Promise<void> {
    return Promise.resolve();
  }
}

/** Records when it was asked, so "was it asked at all" is answerable. */
class RecordingPrices implements ReservePriceStore {
  pricesByReserve = new Map<string, ReservePrice>();
  calls = 0;

  latest(): Promise<ReadonlyMap<string, ReservePrice>> {
    this.calls += 1;
    return Promise.resolve(this.pricesByReserve);
  }

  put(): Promise<void> {
    return Promise.resolve();
  }
}

function priced(price = USDC_PRICE, ageSeconds = 41): ReservePrice {
  return { price, pricedAt: PRICED_AT, ageSeconds };
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

function serviceWith(
  store: PositionStore,
  sync: SyncStatusStore,
  tokens: TokenMetadataStore = new RecordingTokens(),
  prices: ReservePriceStore = new RecordingPrices(),
): PositionsService {
  return new PositionsService(store, tokens, prices, sync, STALE_AFTER, PRICE_STALE_AFTER, cursors);
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
        asset: { assetId: '7', hub: HUB, underlying: USDC, decimals: 6, symbol: null, name: null },
        value: {
          suppliedAmount: '1000000000',
          drawnDebt: '0',
          premiumDebt: '0',
          totalDebt: '0',
          drawnIndex: RAY,
          price: null,
          suppliedValue: null,
          debtValue: null,
        },
      },
    ]);
  });

  describe('labels', () => {
    it('joins a label onto the asset it belongs to', async () => {
      const tokens = new RecordingTokens();
      tokens.labelsByToken.set(USDC, { symbol: 'USDC', name: 'USD Coin' });
      store.page = { valuedAt: VALUED_AT, items: [position()], next: null };

      const { items } = await serviceWith(store, new FixedSync(syncAt(7)), tokens).list(
        { chainId: CHAIN_ID, user: ALICE },
        { limit: 50 },
      );

      expect(items[0]?.asset).toMatchObject({ symbol: 'USDC', name: 'USD Coin' });
    });

    it('serves null for a token enrichment has not reached', async () => {
      store.page = { valuedAt: VALUED_AT, items: [position()], next: null };

      // Absent from the map. Distinct in the store from "asked, and it has no
      // symbol", which is why the sweep can tell them apart — but the wire
      // cannot express the difference and a caller has no use for it.
      const { items } = await list();
      expect(items[0]?.asset).toMatchObject({ symbol: null, name: null });
    });

    it('asks for labels without waiting for the page', async () => {
      // The property the two-database split rests on: labels are keyed by
      // chain alone, so they do not depend on which positions come back. If
      // this ever became sequential the second round trip would stop being
      // free, and nothing else would notice.
      const tokens = new RecordingTokens();
      let releasePage!: () => void;
      const slowPage = new Promise<void>((resolve) => {
        releasePage = resolve;
      });

      const blocking: PositionStore = {
        list: async () => {
          await slowPage;
          return store.page;
        },
        holdings: () => Promise.resolve({ items: store.page.items, valuedAt: VALUED_AT }),
      };

      const pending = serviceWith(blocking, new FixedSync(syncAt(7)), tokens).list(
        { chainId: CHAIN_ID, user: ALICE },
        { limit: 50 },
      );

      // Asked while the page is still blocked. Sequential code would not have
      // reached this call yet.
      await Promise.resolve();
      expect(tokens.calls).toBe(1);

      releasePage();
      await pending;
    });
  });

  describe('prices', () => {
    /**
     * Rebuilds the subject with the given prices already stored.
     *
     * Reassigns the suite's `service` rather than returning a second one, so
     * every test here still goes through the same `list` helper as the rest of
     * the file — one way in, whatever is being set up.
     */
    function withPrices(entries: [string, ReservePrice][]): RecordingPrices {
      const prices = new RecordingPrices();
      for (const [reserveId, price] of entries) {
        prices.pricesByReserve.set(reserveKey(SPOKE, reserveId), price);
      }
      service = serviceWith(store, new FixedSync(syncAt(7)), new RecordingTokens(), prices);
      return prices;
    }

    const page = (over: Partial<Position> = {}): PositionPage => ({
      valuedAt: VALUED_AT,
      items: [position(over)],
      next: null,
    });

    it('values an amount in the protocol’s own unit', async () => {
      store.page = page();
      withPrices([['7', priced()]]);

      const { items } = await list();

      // §7.1: `amount × price × 10^(18 − dec)`, where 1e26 is one dollar.
      // 1000000000 (6dp USDC) × 99971505 (8dp) × 1e12 = 1000 USDC ≈ $999.71.
      expect(items[0]?.value).toMatchObject({
        price: USDC_PRICE,
        suppliedValue: '99971505000000000000000000000',
        debtValue: '0',
      });
    });

    it('prices the debt from the rounded token amount, not the shares', async () => {
      store.page = page({
        value: {
          suppliedAmount: '0',
          drawnDebt: '500000000',
          premiumDebt: '1',
          totalDebt: '500000001',
          drawnIndex: RAY,
        },
      });
      withPrices([['7', priced()]]);

      const { items } = await list();

      // `totalDebt`, which is what is owed. The health factor deliberately does
      // not reuse this — it divides an unrounded ray-scaled debt — so the two
      // are meant to differ in the last digits.
      expect(items[0]?.value?.debtValue).toBe((500_000_001n * 99_971_505n * 10n ** 12n).toString());
    });

    it('serves null rather than zero for a reserve with no price', async () => {
      store.page = page();
      withPrices([]);

      const { items, pricing } = await list();

      // A zero would be indistinguishable from a real one — and the oracle
      // reverts rather than answer zero (§7.4), so a real one cannot occur.
      expect(items[0]?.value).toMatchObject({
        price: null,
        suppliedValue: null,
        debtValue: null,
      });
      expect(pricing).toBeNull();
    });

    it('keeps two spokes’ reserve 7 apart', async () => {
      // Each Spoke has its own oracle over its own id space (§12.3). Keying on
      // reserveId alone would price one spoke's position with another's feed.
      const other = '0x973a023a77420ba610f06b3858ad991df6d85a08';
      store.page = { valuedAt: VALUED_AT, items: [position({ spoke: other })], next: null };
      withPrices([['7', priced()]]);

      const { items } = await list();

      expect(items[0]?.value?.price).toBeNull();
    });

    it('reports the oldest price behind the page', async () => {
      store.page = {
        valuedAt: VALUED_AT,
        items: [position(), position({ reserveId: '9' })],
        next: null,
      };
      withPrices([
        ['7', priced(USDC_PRICE, 12)],
        ['9', priced(USDC_PRICE, 900)],
      ]);

      const { pricing } = await list();

      // The worst number in front of the caller, not the best or the mean.
      // Prices are normally written in one upsert and agree; they diverge
      // exactly when the oracle refused one and its last price was left to age.
      expect(pricing).toEqual({
        updatedAt: PRICED_AT.toISOString(),
        ageSeconds: 900,
        stale: true,
      });
    });

    it('ignores the age of a price nothing on the page used', async () => {
      store.page = page();
      withPrices([
        ['7', priced(USDC_PRICE, 12)],
        ['99', priced(USDC_PRICE, 9_000)],
      ]);

      const { pricing } = await list();

      expect(pricing).toMatchObject({ ageSeconds: 12, stale: false });
    });

    it('serves no prices at all when asOf is set', async () => {
      store.page = page();
      const prices = withPrices([['7', priced()]]);

      const { items, pricing } = await list({ limit: 50, asOf: VALUED_AT - 3_600 });

      // Amounts are extrapolated to that instant and the stored price is
      // whatever the oracle last said, which is now. Pricing one against the
      // other is a number that was never true.
      expect(pricing).toBeNull();
      expect(items[0]?.value).toMatchObject({ price: null, suppliedValue: null });
      // Not fetched and discarded — not fetched.
      expect(prices.calls).toBe(0);
    });

    it('still prices when asOf is merely absent', async () => {
      // The store defaults `asOf` to now when the caller omits it, and that
      // default is still "current" — the test has to be on the parameter, not
      // on the resulting timestamp.
      store.page = page();
      const prices = withPrices([['7', priced()]]);

      const { pricing } = await list();

      expect(prices.calls).toBe(1);
      expect(pricing).not.toBeNull();
    });

    it('asks for prices without waiting for the page', async () => {
      const prices = new RecordingPrices();
      let releasePage!: () => void;
      const slowPage = new Promise<void>((resolve) => {
        releasePage = resolve;
      });

      const blocking: PositionStore = {
        list: async () => {
          await slowPage;
          return store.page;
        },
        holdings: () => Promise.resolve({ items: store.page.items, valuedAt: VALUED_AT }),
      };

      const pending = serviceWith(
        blocking,
        new FixedSync(syncAt(7)),
        new RecordingTokens(),
        prices,
      ).list({ chainId: CHAIN_ID, user: ALICE }, { limit: 50 });

      // Three reads, one round trip. Sequential code would not have reached
      // this call yet.
      await Promise.resolve();
      expect(prices.calls).toBe(1);

      releasePage();
      await pending;
    });
  });
  describe('portfolio', () => {
    const SECOND_SPOKE = '0x973a023a77420ba610f06b3858ad991df6d85a08';

    /** Rebuilds the subject with the given prices, as the pricing suite does. */
    function withPrices(entries: [string, ReservePrice][]): RecordingPrices {
      const prices = new RecordingPrices();
      for (const [reserveId, price] of entries) {
        prices.pricesByReserve.set(reserveKey(SPOKE, reserveId), price);
      }
      service = serviceWith(store, new FixedSync(syncAt(7)), new RecordingTokens(), prices);
      return prices;
    }

    /** A position with a supply and a debt, both priceable. */
    function held(over: Partial<Position> = {}): Position {
      return position({
        value: {
          suppliedAmount: '1000000000',
          drawnDebt: '400000000',
          premiumDebt: '0',
          totalDebt: '400000000',
          drawnIndex: RAY,
        },
        ...over,
      });
    }

    it('totals supply, debt and the difference between them', async () => {
      store.page = { valuedAt: VALUED_AT, items: [held()], next: null };
      withPrices([['7', priced()]]);

      const { portfolio } = await list();

      const supplied = 1_000_000_000n * 99_971_505n * 10n ** 12n;
      const debt = 400_000_000n * 99_971_505n * 10n ** 12n;
      expect(portfolio).toEqual([
        {
          spoke: SPOKE,
          suppliedValue: supplied.toString(),
          debtValue: debt.toString(),
          netWorth: (supplied - debt).toString(),
        },
      ]);
    });

    it('keeps two Spokes apart rather than summing them', async () => {
      // §12.3 is structural: two Spokes are two independent margin accounts, and
      // one blended figure hides an imminent liquidation behind unrelated
      // collateral. There is no portfolio-wide total, by construction.
      store.page = {
        valuedAt: VALUED_AT,
        items: [held(), held({ spoke: SECOND_SPOKE })],
        next: null,
      };
      const prices = withPrices([['7', priced()]]);
      prices.pricesByReserve.set(reserveKey(SECOND_SPOKE, '7'), priced('200000000'));

      const { portfolio } = await list();

      expect(portfolio).toHaveLength(2);
      expect(portfolio?.map((entry) => entry.spoke)).toEqual([SPOKE, SECOND_SPOKE]);
      expect(portfolio?.[0]?.suppliedValue).not.toBe(portfolio?.[1]?.suppliedValue);
    });

    it('reports a negative net worth rather than clamping it', async () => {
      store.page = {
        valuedAt: VALUED_AT,
        items: [
          held({
            value: {
              suppliedAmount: '1',
              drawnDebt: '1000000000',
              premiumDebt: '0',
              totalDebt: '1000000000',
              drawnIndex: RAY,
            },
          }),
        ],
        next: null,
      };
      withPrices([['7', priced()]]);

      const { portfolio } = await list();

      // A wallet whose debt has outgrown its collateral is exactly the case
      // worth seeing, and a zero would read as "nothing here".
      expect(portfolio?.[0]?.netWorth?.startsWith('-')).toBe(true);
    });

    it('nulls a Spoke where anything could not be priced', async () => {
      store.page = {
        valuedAt: VALUED_AT,
        items: [held(), held({ reserveId: '9' })],
        next: null,
      };
      withPrices([['7', priced()]]);

      const { portfolio } = await list();

      // Leaving reserve 9 out would understate the total silently. Null is the
      // same refusal `asset` and `value` make when the join has nothing.
      expect(portfolio).toEqual([
        { spoke: SPOKE, suppliedValue: null, debtValue: null, netWorth: null },
      ]);
    });

    it('totals the whole listing, not the page in front of the caller', async () => {
      // The property the second read exists for. A total over a page is
      // arithmetic on a subset and looks like a whole number.
      store.page = {
        valuedAt: VALUED_AT,
        items: [held()],
        next: { spoke: SPOKE, reserveId: '7' },
      };
      store.beyondThePage = [held(), held({ reserveId: '9' })];
      const prices = withPrices([['7', priced()]]);
      prices.pricesByReserve.set(reserveKey(SPOKE, '9'), priced());

      const { portfolio, items } = await list({ limit: 1 });

      expect(items).toHaveLength(1);
      const one = 1_000_000_000n * 99_971_505n * 10n ** 12n;
      expect(portfolio?.[0]?.suppliedValue).toBe((one * 2n).toString());
    });

    it('narrows with the Spoke filter', async () => {
      store.page = { valuedAt: VALUED_AT, items: [held()], next: null };
      withPrices([['7', priced()]]);

      await list({ limit: 50, spoke: SPOKE });

      expect(store.holdingQueries[0]).toEqual({ chainId: CHAIN_ID, user: ALICE, spoke: SPOKE });
    });

    it('carries no page size or cursor into the unpaged read', async () => {
      store.page = { valuedAt: VALUED_AT, items: [held()], next: null };
      withPrices([['7', priced()]]);

      await list({ limit: 1 });

      // Paging the totals query would be the bug it exists to avoid.
      expect(store.holdingQueries[0]).not.toHaveProperty('limit');
      expect(store.holdingQueries[0]).not.toHaveProperty('after');
    });

    it('is null when asOf is set, and is not even asked for', async () => {
      store.page = { valuedAt: VALUED_AT, items: [held()], next: null };
      withPrices([['7', priced()]]);

      const { portfolio } = await list({ limit: 50, asOf: VALUED_AT - 3_600 });

      // Totals are sums of prices, and there are none on a historical query.
      expect(portfolio).toBeNull();
      expect(store.holdingQueries).toEqual([]);
    });

    it('is an empty list rather than null for a wallet holding nothing', async () => {
      withPrices([]);

      const { portfolio } = await list();

      // Null means "not priced at all"; empty means "priced, and there is
      // nothing". A caller branching on one must not get the other.
      expect(portfolio).toEqual([]);
    });
  });
});
