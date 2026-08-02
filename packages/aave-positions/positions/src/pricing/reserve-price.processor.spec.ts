import type { Address, BlockHeader, ChainClient } from '@packages/indexing';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { ReservePrice, ReservePriceRow } from '../store/reserve-price';
import { reserveKey, type ReservePriceStore } from '../store/reserve-price-store';
import type { ReserveListings } from '../store/reserve-listing-source';
import type { ReservePriceReader, ReservePrices } from './reserve-price-reader';
import { ReservePriceProcessor } from './reserve-price.processor';

const CHAIN_ID = 1;
const SPOKE: Address = '0x94e7a5dcbe816e498b89ab752661904e2f56c485';
const ORACLE: Address = '0x99b2b6cea9c3d2fd8f4d90f86741c44b212a6127';
const HEAD = 25_652_782;

/** Retry is the shorter of the two, which several tests below depend on. */
const REFRESH_MS = 60_000;
const RETRY_MS = 15_000;

const NEVER_ABORTED = new AbortController().signal;

/**
 * Drains the microtask queue.
 *
 * The whole point of this processor is that nothing hands the caller a promise
 * to await, so a test cannot wait on the work the way production does not
 * either — it has to let the queue run down instead.
 */
const settle = (): Promise<void> => new Promise((resolve) => setImmediate(resolve));

class FakeListings implements ReserveListings {
  reserveIds: string[] = ['0', '1'];
  calls = 0;

  forSpoke(): Promise<readonly string[]> {
    this.calls += 1;
    return Promise.resolve(this.reserveIds);
  }
}

class FakeStore implements ReservePriceStore {
  readonly rows = new Map<string, ReservePriceRow>();
  /** Every `put`, including empty ones — "did it write nothing?" is a question. */
  readonly puts: (readonly ReservePriceRow[])[] = [];

  latest(): Promise<ReadonlyMap<string, ReservePrice>> {
    return Promise.resolve(new Map<string, ReservePrice>());
  }

  put(rows: readonly ReservePriceRow[]): Promise<void> {
    this.puts.push(rows);
    for (const row of rows) this.rows.set(reserveKey(row.spoke, row.reserveId), row);
    return Promise.resolve();
  }
}

class FakeReader implements ReservePriceReader {
  prices = new Map<string, string>([
    ['0', '187522000000'],
    ['1', '99971505'],
  ]);
  failures: string[] = [];
  throws: Error | null = null;
  readonly seen: { oracle: Address; reserveIds: readonly string[]; atBlock: bigint }[] = [];
  /** Set to hold every read until released, so "did it block?" is answerable. */
  gate: Promise<void> | null = null;

  async read(
    oracle: Address,
    reserveIds: readonly string[],
    atBlock: bigint,
  ): Promise<ReservePrices> {
    this.seen.push({ oracle, reserveIds, atBlock });
    if (this.gate) await this.gate;
    if (this.throws) throw this.throws;
    return { blockNumber: atBlock, prices: this.prices, failures: this.failures };
  }
}

class FakeChain implements ChainClient {
  throws: Error | null = null;

  getChainId(): Promise<number> {
    return Promise.resolve(CHAIN_ID);
  }

  getHeadBlockNumber(): Promise<number> {
    if (this.throws) return Promise.reject(this.throws);
    return Promise.resolve(HEAD);
  }

  getBlockHeader(): Promise<BlockHeader> {
    throw new Error('not used');
  }
}

let listings: FakeListings;
let store: FakeStore;
let reader: FakeReader;
let chain: FakeChain;
let processor: ReservePriceProcessor;

/** One dispatch, then let whatever it started run to completion. */
async function dispatch(from = 100, to = 200): Promise<void> {
  processor.onBlockRange(from, to, NEVER_ABORTED);
  await settle();
}

beforeEach(() => {
  // Only `Date`, so `setImmediate` stays real and `settle` still drains.
  vi.useFakeTimers({ toFake: ['Date'] });
  listings = new FakeListings();
  store = new FakeStore();
  reader = new FakeReader();
  chain = new FakeChain();
  processor = new ReservePriceProcessor(
    { chainId: CHAIN_ID, spoke: SPOKE, oracle: ORACLE, refreshMs: REFRESH_MS, retryMs: RETRY_MS },
    listings,
    store,
    reader,
    chain,
  );
});

afterEach(() => {
  vi.useRealTimers();
});

describe('ReservePriceProcessor', () => {
  describe('staying off the loop', () => {
    it('returns ok before doing any of the work', () => {
      const outcome = processor.onBlockRange(100, 200, NEVER_ABORTED);

      // Not a promise, and nothing read yet. `dispatchToProcessors` runs
      // processors one after another, so awaiting an oracle here would hold the
      // Spoke and Hub ledgers behind it.
      expect(outcome).toEqual({ status: 'ok' });
      expect(reader.seen).toEqual([]);
    });

    it('still returns ok when the oracle throws', async () => {
      reader.throws = new Error('socket hang up');

      const outcome = processor.onBlockRange(100, 200, NEVER_ABORTED);
      await settle();

      // Never `retry`: that would hold the cursor and stop indexing over a
      // price. Never `failed`: that would fail readiness over one.
      expect(outcome).toEqual({ status: 'ok' });
    });

    it('runs one read at a time', async () => {
      let release!: () => void;
      reader.gate = new Promise<void>((resolve) => {
        release = resolve;
      });

      processor.onBlockRange(100, 200, NEVER_ABORTED);
      await settle();
      processor.onBlockRange(201, 300, NEVER_ABORTED);
      await settle();

      expect(reader.seen).toHaveLength(1);

      release();
      await settle();
    });

    it('does not start when the process is already shutting down', async () => {
      const aborted = AbortSignal.abort();

      processor.onBlockRange(100, 200, aborted);
      await settle();

      expect(listings.calls).toBe(0);
    });
  });

  describe('what it reads', () => {
    it('prices every registered reserve at the chain head', async () => {
      await dispatch();

      // The *head*, not the range's `to`. During a backfill `to` is historical
      // and a full node cannot serve state there — and a price is a
      // current-value question anyway.
      expect(reader.seen).toEqual([
        { oracle: ORACLE, reserveIds: ['0', '1'], atBlock: BigInt(HEAD) },
      ]);
    });

    it('stores what came back', async () => {
      await dispatch();

      expect(store.rows.get(reserveKey(SPOKE, '0'))).toEqual({
        chainId: CHAIN_ID,
        spoke: SPOKE,
        reserveId: '0',
        price: '187522000000',
      });
    });

    it('leaves out a reserve the oracle refused rather than blanking it', async () => {
      reader.prices = new Map([['0', '187522000000']]);
      reader.failures = ['reserve 1: ContractFunctionRevertedError'];

      await dispatch();

      // Writing a null would turn one broken feed into a missing USD value on
      // a live endpoint. Skipping it leaves the last good price to age, which
      // `pricing.stale` already reports.
      expect(store.puts).toEqual([[expect.objectContaining({ reserveId: '0' })]]);
    });

    it('asks nobody before the Spoke has registered anything', async () => {
      listings.reserveIds = [];

      await dispatch();

      // A cold start, where the `AddReserve` events have not been folded yet.
      // Not a failure — and not a reason to call an oracle about nothing.
      expect(reader.seen).toEqual([]);
      expect(store.puts).toEqual([]);
    });
  });

  describe('the cadence', () => {
    it('holds a good price for the refresh interval', async () => {
      await dispatch();
      expect(reader.seen).toHaveLength(1);

      await dispatch();
      expect(reader.seen).toHaveLength(1);

      vi.setSystemTime(Date.now() + REFRESH_MS);
      await dispatch();
      expect(reader.seen).toHaveLength(2);
    });

    it('comes back sooner after a read that left a price stale', async () => {
      reader.failures = ['reserve 1: ContractFunctionRevertedError'];
      await dispatch();
      expect(reader.seen).toHaveLength(1);

      // The retry window is shorter than the refresh, and this is where that
      // matters: §7.1 weighs collateral against debt, so one price standing
      // still while its neighbours move is worse than all of them being a
      // minute old.
      vi.setSystemTime(Date.now() + RETRY_MS);
      await dispatch();
      expect(reader.seen).toHaveLength(2);
    });

    it('does not come back that soon after a clean read', async () => {
      await dispatch();

      vi.setSystemTime(Date.now() + RETRY_MS);
      await dispatch();

      // The mirror of the test above: if the two delays were the same knob,
      // both would pass and neither would mean anything.
      expect(reader.seen).toHaveLength(1);
    });

    it('backs off when the oracle is unreachable', async () => {
      chain.throws = new Error('socket hang up');

      await dispatch();
      await dispatch();

      // One attempt, not one per dispatch. Without the latch a dead provider
      // is re-asked on every block the loop walks.
      expect(reader.seen).toEqual([]);

      chain.throws = null;
      vi.setSystemTime(Date.now() + RETRY_MS);
      await dispatch();
      expect(reader.seen).toHaveLength(1);
    });
  });

  describe('reorgs', () => {
    it('does nothing, because a fork does not change what an asset is worth', async () => {
      expect(processor.onReorg()).toEqual({ status: 'ok' });
      await settle();

      expect(reader.seen).toEqual([]);
      expect(store.puts).toEqual([]);
    });
  });
});
