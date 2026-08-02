import type { Address, BlockHeader, ChainClient } from '@packages/indexing';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { ReservePrice, ReservePriceRow } from './store/reserve-price';
import { reserveKey, type ReservePriceStore } from './store/reserve-price-store';
import type { ReserveListings } from './store/reserve-listing-source';
import type { ReservePriceReader, ReservePrices } from './oracle/reserve-price-reader';
import { ReservePriceRefresher } from './reserve-price.refresher';

const CHAIN_ID = 1;
const SPOKE: Address = '0x94e7a5dcbe816e498b89ab752661904e2f56c485';
const ORACLE: Address = '0x99b2b6cea9c3d2fd8f4d90f86741c44b212a6127';
const HEAD = 25_652_782;

/** Retry is the shorter of the two, which several tests below depend on. */
const REFRESH_MS = 60_000;
const RETRY_MS = 15_000;

class FakeListings implements ReserveListings {
  reserveIds: string[] = ['0', '1'];

  forSpoke(): Promise<readonly string[]> {
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
  readonly seen: { oracle: Address; reserveIds: readonly string[]; atBlock: bigint }[] = [];
  /** Set to hold every read until released, so "did it overlap?" is answerable. */
  gate: Promise<void> | null = null;

  async read(
    oracle: Address,
    reserveIds: readonly string[],
    atBlock: bigint,
  ): Promise<ReservePrices> {
    this.seen.push({ oracle, reserveIds, atBlock });
    if (this.gate) await this.gate;
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

function refresher(autoStart = true): ReservePriceRefresher {
  return new ReservePriceRefresher(
    {
      chainId: CHAIN_ID,
      spoke: SPOKE,
      oracle: ORACLE,
      refreshMs: REFRESH_MS,
      retryMs: RETRY_MS,
      autoStart,
    },
    listings,
    store,
    reader,
    chain,
  );
}

/** Boots one and lets the immediate first read run to completion. */
async function started(): Promise<ReservePriceRefresher> {
  const subject = refresher();
  subject.onApplicationBootstrap();
  await vi.advanceTimersByTimeAsync(0);
  return subject;
}

beforeEach(() => {
  vi.useFakeTimers();
  listings = new FakeListings();
  store = new FakeStore();
  reader = new FakeReader();
  chain = new FakeChain();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('ReservePriceRefresher', () => {
  describe('what drives it', () => {
    it('refreshes on the clock alone, with nothing else happening', async () => {
      // **The property the whole class exists for.** There is no block range,
      // no dispatch and no loop in this test — only time passing. As a
      // `BlockProcessor` this was gated on the indexer having a range to hand
      // out, so a chain gone quiet, an indexer that had stalled, or a pod
      // booted with `INDEXER_AUTOSTART=false` all froze the prices.
      const subject = await started();
      expect(reader.seen).toHaveLength(1);

      await vi.advanceTimersByTimeAsync(REFRESH_MS);
      expect(reader.seen).toHaveLength(2);

      await vi.advanceTimersByTimeAsync(REFRESH_MS);
      expect(reader.seen).toHaveLength(3);

      await subject.onApplicationShutdown();
    });

    it('reads at boot rather than one interval later', async () => {
      // Otherwise every restart serves no price at all until the first tick,
      // and `pricing` would be null on a freshly deployed pod.
      const subject = refresher();
      subject.onApplicationBootstrap();
      expect(reader.seen).toEqual([]);

      await vi.advanceTimersByTimeAsync(0);
      expect(reader.seen).toHaveLength(1);

      await subject.onApplicationShutdown();
    });

    it('stays still when it was told not to start', async () => {
      // How a graph that wants the ports without the poller declines it — the
      // one-shot command, and every hermetic test.
      const subject = refresher(false);
      subject.onApplicationBootstrap();

      await vi.advanceTimersByTimeAsync(REFRESH_MS * 3);

      expect(reader.seen).toEqual([]);
      await subject.onApplicationShutdown();
    });

    it('counts the next delay from when a read finished, not when it began', async () => {
      // A self-rescheduling timeout rather than an interval: a provider slower
      // than the interval cannot leave two reads in flight, so there is no
      // in-flight latch to get wrong.
      let release!: () => void;
      reader.gate = new Promise<void>((resolve) => {
        release = resolve;
      });

      const subject = refresher();
      subject.onApplicationBootstrap();
      await vi.advanceTimersByTimeAsync(0);
      expect(reader.seen).toHaveLength(1);

      // Longer than the whole refresh interval, still inside the first read.
      await vi.advanceTimersByTimeAsync(REFRESH_MS * 2);
      expect(reader.seen).toHaveLength(1);

      release();
      reader.gate = null;
      await vi.advanceTimersByTimeAsync(0);
      await vi.advanceTimersByTimeAsync(REFRESH_MS);
      expect(reader.seen).toHaveLength(2);

      await subject.onApplicationShutdown();
    });
  });

  describe('the two delays', () => {
    it('comes back sooner after a read that left a price stale', async () => {
      reader.failures = ['reserve 1: ContractFunctionRevertedError'];
      const subject = await started();
      expect(reader.seen).toHaveLength(1);

      // §7.1 weighs collateral against debt, so one price standing still while
      // its neighbours move is worse than all of them ageing together.
      await vi.advanceTimersByTimeAsync(RETRY_MS);
      expect(reader.seen).toHaveLength(2);

      await subject.onApplicationShutdown();
    });

    it('does not come back that soon after a clean read', async () => {
      const subject = await started();

      await vi.advanceTimersByTimeAsync(RETRY_MS);

      // The mirror of the test above: if the two delays were one knob, both
      // would pass and neither would mean anything.
      expect(reader.seen).toHaveLength(1);
      await subject.onApplicationShutdown();
    });

    it('backs off and recovers when the chain is unreachable', async () => {
      chain.throws = new Error('socket hang up');
      const subject = await started();
      expect(reader.seen).toEqual([]);

      chain.throws = null;
      await vi.advanceTimersByTimeAsync(RETRY_MS);

      expect(reader.seen).toHaveLength(1);
      await subject.onApplicationShutdown();
    });
  });

  describe('what it reads and writes', () => {
    it('prices every registered reserve at the chain head', async () => {
      const subject = await started();

      // The head, not a historical block: a price is a current-value question,
      // and pinning is what stops fourteen reads landing at fourteen heights.
      expect(reader.seen).toEqual([
        { oracle: ORACLE, reserveIds: ['0', '1'], atBlock: BigInt(HEAD) },
      ]);
      expect(store.rows.get(reserveKey(SPOKE, '0'))).toEqual({
        chainId: CHAIN_ID,
        spoke: SPOKE,
        reserveId: '0',
        price: '187522000000',
      });

      await subject.onApplicationShutdown();
    });

    it('leaves out a reserve the oracle refused rather than blanking it', async () => {
      reader.prices = new Map([['0', '187522000000']]);
      reader.failures = ['reserve 1: ContractFunctionRevertedError'];

      const subject = await started();

      // Writing a null would turn one broken feed into a missing USD value on
      // a live endpoint. Skipping it leaves the last good price to age.
      expect(store.puts).toEqual([[expect.objectContaining({ reserveId: '0' })]]);
      await subject.onApplicationShutdown();
    });

    it('asks nobody before the Spoke has registered anything', async () => {
      listings.reserveIds = [];

      const subject = await started();

      // A cold start, where `AddReserve` has not been folded yet. Not a failure
      // — and not a reason to call an oracle about nothing.
      expect(reader.seen).toEqual([]);
      expect(store.puts).toEqual([]);
      await subject.onApplicationShutdown();
    });
  });

  describe('shutdown', () => {
    it('stops the timer', async () => {
      const subject = await started();

      await subject.onApplicationShutdown();
      await vi.advanceTimersByTimeAsync(REFRESH_MS * 3);

      // Without this the process keeps polling an oracle after the pod has been
      // told to drain, and keeps a database pool open to write the answer into.
      expect(reader.seen).toHaveLength(1);
    });

    it('lets a read already in flight store what it paid for', async () => {
      let release!: () => void;
      reader.gate = new Promise<void>((resolve) => {
        release = resolve;
      });

      const subject = refresher();
      subject.onApplicationBootstrap();
      await vi.advanceTimersByTimeAsync(0);
      expect(store.puts).toEqual([]);

      const draining = subject.onApplicationShutdown();
      release();
      await draining;

      // The `eth_call` is already spent. Abandoning the answer means making the
      // same call again on the next start.
      expect(store.puts).toHaveLength(1);
    });
  });
});
