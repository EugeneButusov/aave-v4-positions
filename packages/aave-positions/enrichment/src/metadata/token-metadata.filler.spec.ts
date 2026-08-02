import type {
  Address,
  BlockHeader,
  ChainClient,
  Erc20MetadataReader,
  TokenMetadata,
} from '@packages/indexing';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { TokenListings } from '../store/token-listing-source';
import { PendingTokens } from './pending-tokens';
import type { TokenLabel, TokenMetadataRow } from '../store/token-metadata';
import type { TokenMetadataStore } from '../store/token-metadata-store';
import { TokenMetadataFiller } from './token-metadata.filler';

const CHAIN_ID = 1;
const RETRY_MS = 60_000;
const HEAD = 25_652_782;

const USDC = '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48';
const WETH = '0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2';
const DAI = '0x6b175474e89094c44da98b954eedeac495271d0f';

function metadata(over: Partial<TokenMetadata> = {}): TokenMetadata {
  return { symbol: 'USDC', name: 'USD Coin', decimals: 6, failures: [], ...over };
}

/**
 * Drains the microtask queue.
 *
 * The whole point of this filler is that nothing hands the caller a promise
 * to await, so a test cannot wait on the work the way production does not
 * either — it has to let the queue run down instead. `setImmediate` yields past
 * every pending microtask chain in one go.
 */
const settle = (): Promise<void> => new Promise((resolve) => setImmediate(resolve));

class FakeListings implements TokenListings {
  listed: Address[] = [];
  /** How many times the whole set was asked for — the only query there is. */
  calls = 0;

  all(): Promise<readonly Address[]> {
    this.calls += 1;
    return Promise.resolve(this.listed);
  }
}

class FakeStore implements TokenMetadataStore {
  readonly rows = new Map<Address, TokenMetadataRow>();
  labelCalls = 0;

  labels(): Promise<ReadonlyMap<Address, TokenLabel>> {
    this.labelCalls += 1;
    return Promise.resolve(
      new Map(
        [...this.rows].map(([token, row]) => [token, { symbol: row.symbol, name: row.name }]),
      ),
    );
  }

  put(rows: readonly TokenMetadataRow[]): Promise<void> {
    for (const row of rows) this.rows.set(row.token, row);
    return Promise.resolve();
  }
}

class FakeReader implements Erc20MetadataReader {
  answers = new Map<Address, TokenMetadata>();
  throws: Error | null = null;
  readonly seen: { token: Address; atBlock: bigint }[] = [];
  /** Set to hold every read until released, so "did it block?" is answerable. */
  gate: Promise<void> | null = null;

  async read(token: Address, atBlock: bigint): Promise<TokenMetadata> {
    this.seen.push({ token, atBlock });
    if (this.gate) await this.gate;
    if (this.throws) throw this.throws;
    return this.answers.get(token) ?? metadata();
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
let pending: PendingTokens;
let store: FakeStore;
let reader: FakeReader;
let chain: FakeChain;
let filler: TokenMetadataFiller;

function build(concurrency = 4, autoStart = true): TokenMetadataFiller {
  return new TokenMetadataFiller(
    { chainId: CHAIN_ID, retryDelayMs: RETRY_MS, concurrency, autoStart },
    listings,
    pending,
    store,
    reader,
    chain,
  );
}

describe('TokenMetadataFiller', () => {
  beforeEach(() => {
    // `setTimeout` as well as `Date`, because the back-off is now a timer this
    // class owns rather than a gate on the next dispatch. `setImmediate` stays
    // real: `settle` depends on it, and faking it would deadlock every test.
    vi.useFakeTimers({ toFake: ['Date', 'setTimeout', 'clearTimeout'] });
    listings = new FakeListings();
    pending = new PendingTokens();
    store = new FakeStore();
    reader = new FakeReader();
    chain = new FakeChain();
    filler = build();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  describe('what starts it', () => {
    it('returns from bootstrap before doing any of the work', () => {
      listings.listed = [USDC];

      const outcome = filler.onApplicationBootstrap();

      // Not `async`, and nothing awaited: `add()` is called from the ingestion
      // path, and a listing must never wait on seventeen ERC-20 reads to
      // finish being recorded.
      expect(outcome).toBeUndefined();
      expect(store.rows.size).toBe(0);
    });

    it('reads without any block ever being dispatched', async () => {
      // **The property the class exists for.** There is no loop in this test.
      // As a `BlockProcessor` the first full check waited for a dispatch, so a
      // pod booted with `INDEXER_AUTOSTART=false` read nothing at all.
      listings.listed = [USDC];

      filler.onApplicationBootstrap();
      await settle();

      expect(store.rows.has(USDC)).toBe(true);
    });

    it('starts nothing while a run is already in flight', async () => {
      listings.listed = [USDC];
      let release!: () => void;
      reader.gate = new Promise<void>((resolve) => {
        release = resolve;
      });

      filler.onApplicationBootstrap();
      await settle();
      pending.add([WETH]);
      pending.add([DAI]);
      await settle();

      // Two listings in one block would otherwise pile two more fan-outs onto
      // the same provider while the first is still outstanding.
      expect(listings.calls).toBe(1);

      release();
      await settle();
    });

    it('survives a reader that throws', async () => {
      listings.listed = [USDC];
      reader.throws = new Error('provider exploded');

      filler.onApplicationBootstrap();
      await settle();

      expect(store.rows.size).toBe(0);
    });

    it('survives a chain head that cannot be read', async () => {
      listings.listed = [USDC];
      chain.throws = new Error('no providers');

      filler.onApplicationBootstrap();
      await settle();

      expect(store.rows.size).toBe(0);
    });

    it('does nothing at all when it was told not to start', async () => {
      listings.listed = [USDC];
      const idle = build(4, false);

      idle.onApplicationBootstrap();
      pending.add([WETH]);
      await settle();

      // How `enrich:tokens` and every hermetic test decline the worker.
      expect(listings.calls).toBe(0);
      expect(reader.seen).toEqual([]);
    });

    it('stops on shutdown, and a later listing does not restart it', async () => {
      listings.listed = [USDC];
      filler.onApplicationBootstrap();
      await settle();

      await filler.onApplicationShutdown();
      pending.add([WETH]);
      await settle();

      expect(store.rows.has(WETH)).toBe(false);
    });

    it('lets a read already in flight store what it paid for', async () => {
      listings.listed = [USDC];
      let release!: () => void;
      reader.gate = new Promise<void>((resolve) => {
        release = resolve;
      });

      filler.onApplicationBootstrap();
      await settle();
      expect(store.rows.size).toBe(0);

      const draining = filler.onApplicationShutdown();
      release();
      await draining;

      // The `eth_call`s are already spent; abandoning the answers means making
      // them again on the next start.
      expect(store.rows.has(USDC)).toBe(true);
    });
  });

  describe('what wakes it up', () => {
    it('asks for the whole listing set the first time, because nothing has been checked', async () => {
      listings.listed = [USDC];

      filler.onApplicationBootstrap();
      await settle();

      // Every `AddAsset` on mainnet fired at block 24,722,784, far behind any
      // live cursor, so nothing pushes those tokens to a fresh indexer. It is
      // also what covers the buffer being in memory: a restart loses it, and
      // this is the recovery.
      expect(listings.calls).toBe(1);
      expect(store.rows.has(USDC)).toBe(true);
    });

    it('sleeps once it has caught up, with no timer left running', async () => {
      listings.listed = [USDC];
      filler.onApplicationBootstrap();
      await settle();
      const queriesAfterCatchUp = listings.calls;
      const labelsAfterCatchUp = store.labelCalls;

      // Hours of wall clock, and nothing listed. A clean run arms no timer at
      // all — the buffer is what wakes it, so there is nothing to poll.
      await vi.advanceTimersByTimeAsync(RETRY_MS * 100);

      expect(listings.calls).toBe(queriesAfterCatchUp);
      expect(store.labelCalls).toBe(labelsAfterCatchUp);
      expect(reader.seen).toHaveLength(1);
    });

    it('enriches a token the moment ingestion pushes it', async () => {
      listings.listed = [USDC];
      filler.onApplicationBootstrap();
      await settle();
      const queriesAfterCatchUp = listings.calls;

      // What the Hub filler does as it writes the `AddAsset`. The buffer
      // wakes the filler directly — no dispatch, no timer, no query.
      pending.add([WETH]);
      await settle();

      expect(store.rows.has(WETH)).toBe(true);
      expect(listings.calls).toBe(queriesAfterCatchUp);
    });

    it('ignores a push of something it already holds', async () => {
      listings.listed = [USDC];
      filler.onApplicationBootstrap();
      await settle();
      const reads = reader.seen.length;

      // `add` only wakes on an address the set did not already have, so a range
      // re-dispatched after a retry does not become a second fan-out.
      pending.add([USDC]);
      pending.add([USDC]);
      await settle();

      expect(reader.seen).toHaveLength(reads);
    });

    it('drains the buffer even when it is doing a full check', async () => {
      listings.listed = [USDC];
      pending.add([USDC]);

      filler.onApplicationBootstrap();
      await settle();

      // Left buffered, USDC would wake it again for work this run has already
      // done — a poll again, just a slower one.
      expect(pending.size).toBe(0);
      expect(listings.calls).toBe(1);
    });

    it('still enriches a token whose push was lost to a restart', async () => {
      // The buffer is in memory, so a crash between the push and the run drops
      // it. That is safe because the buffer is never the only record: the Hub
      // filler committed the `AddAsset` to the ledger *before* pushing, so
      // the listing survives, and a fresh instance owes a full check that
      // finds the gap between the ledger and the store.
      pending.add([WETH]);

      const afterRestart = new TokenMetadataFiller(
        { chainId: CHAIN_ID, retryDelayMs: RETRY_MS, concurrency: 4, autoStart: true },
        listings,
        new PendingTokens(),
        store,
        reader,
        chain,
      );
      listings.listed = [WETH];

      afterRestart.onApplicationBootstrap();
      await settle();

      expect(store.rows.has(WETH)).toBe(true);
    });

    it('goes back to the whole set after a failure, not to the buffer', async () => {
      // Deliberately after a *successful* run, which is the only way to tell
      // the two apart: on a cold start the flag is already set, so a mutation
      // that never sets it would still look correct there.
      listings.listed = [USDC];
      filler.onApplicationBootstrap();
      await settle();
      expect(listings.calls).toBe(1);

      // A listing lands, and the token cannot be reached.
      reader.answers.set(WETH, {
        symbol: null,
        name: null,
        decimals: null,
        failures: ['symbol: HttpRequestError'],
      });
      pending.add([WETH]);
      await settle();
      expect(listings.calls).toBe(1);
      expect(store.rows.has(WETH)).toBe(false);

      // The retry cannot use the buffer — WETH was drained out of it — so it
      // has to re-derive the set or the gap never closes.
      listings.listed = [USDC, WETH];
      reader.answers.delete(WETH);
      await vi.advanceTimersByTimeAsync(RETRY_MS);
      await settle();

      expect(listings.calls).toBe(2);
      expect(store.rows.has(WETH)).toBe(true);
    });
  });

  describe('backing off', () => {
    it('is not woken out of a back-off by a new listing', async () => {
      listings.listed = [USDC];
      chain.throws = new Error('no providers');

      filler.onApplicationBootstrap();
      await settle();
      expect(listings.calls).toBe(1);

      pending.add([WETH]);
      await settle();

      // A dead provider is retried on the schedule, not once per `AddAsset`.
      // Nothing is lost by ignoring the push: the armed retry re-derives the
      // whole set, so it covers the new token too.
      expect(listings.calls).toBe(1);
    });

    it('tries again once the delay has passed', async () => {
      listings.listed = [USDC];
      chain.throws = new Error('no providers');

      filler.onApplicationBootstrap();
      await settle();

      chain.throws = null;
      // Its own timer, so nothing outside has to happen for the retry to fire.
      await vi.advanceTimersByTimeAsync(RETRY_MS);
      await settle();

      expect(store.rows.has(USDC)).toBe(true);
    });

    it('does not back off when there was simply nothing to do', async () => {
      listings.listed = [USDC];
      filler.onApplicationBootstrap();
      await settle();

      pending.add([WETH]);
      filler.onApplicationBootstrap();
      await settle();

      // A successful run imposes no delay, so a new listing is picked up on the
      // very next dispatch rather than waiting out a timer it did not earn.
      expect(store.rows.has(WETH)).toBe(true);
    });
  });

  describe('what it stores', () => {
    it('enriches every listed token the store does not have', async () => {
      listings.listed = [USDC, WETH];

      filler.onApplicationBootstrap();
      await settle();

      expect([...store.rows.keys()].toSorted()).toEqual([USDC, WETH].toSorted());
      expect(store.rows.get(USDC)?.fetchedAtBlock).toBe(HEAD);
    });

    it('skips a token it already has', async () => {
      listings.listed = [USDC];
      pending.add([USDC]);
      filler.onApplicationBootstrap();
      await settle();

      filler.onApplicationBootstrap();
      await settle();

      expect(reader.seen).toHaveLength(1);
    });

    it('asks the store nothing when the Hub has listed nothing', async () => {
      filler.onApplicationBootstrap();
      await settle();

      expect(store.labelCalls).toBe(0);
    });

    it('writes a row for a token that has no symbol at all', async () => {
      listings.listed = [USDC];
      reader.answers.set(USDC, {
        symbol: null,
        name: null,
        decimals: null,
        failures: [
          'symbol: ContractFunctionZeroDataError',
          'name: ContractFunctionZeroDataError',
          'decimals: ContractFunctionZeroDataError',
        ],
      });

      filler.onApplicationBootstrap();
      await settle();

      // The row existing is what records that the question was put. Without it
      // a conformant token with no optional metadata is re-read forever.
      expect(store.rows.get(USDC)).toMatchObject({ symbol: null, name: null });
    });

    it('leaves the gap open when the node could not be reached', async () => {
      listings.listed = [USDC];
      reader.answers.set(USDC, {
        symbol: null,
        name: null,
        decimals: null,
        failures: ['symbol: HttpRequestError', 'name: HttpRequestError', 'decimals: TimeoutError'],
      });

      filler.onApplicationBootstrap();
      await settle();

      // The other half of the rule above, and the trap it exists to avoid: a
      // timeout written as a null closes the gap on a label nobody will revisit.
      expect(store.rows.has(USDC)).toBe(false);
    });

    it('treats a value it rejected itself as the token having answered', async () => {
      listings.listed = [USDC];
      reader.answers.set(
        USDC,
        metadata({ decimals: null, failures: ['decimals: out of range (999)'] }),
      );

      filler.onApplicationBootstrap();
      await settle();

      expect(store.rows.get(USDC)).toMatchObject({ symbol: 'USDC', tokenDecimals: null });
    });

    it('reads at the chain head, not at the range it was handed', async () => {
      listings.listed = [USDC];

      filler.onApplicationBootstrap();
      await settle();

      // During a backfill the range is historical and a full node cannot serve
      // state there — every call would fail, indistinguishably from a token
      // with no symbol.
      expect(reader.seen[0]?.atBlock).toBe(BigInt(HEAD));
    });

    it('keeps what it already read when shutdown lands mid-run', async () => {
      const shutdown = new AbortController();
      listings.listed = [USDC];
      let release!: () => void;
      reader.gate = new Promise<void>((resolve) => {
        release = resolve;
      });

      filler.onApplicationBootstrap();
      await settle();
      shutdown.abort();
      release();
      await settle();

      // The reads are already paid for. Discarding them would mean making the
      // same calls again on the next start.
      expect(store.rows.has(USDC)).toBe(true);
    });
  });

  describe('concurrency', () => {
    it('reads in bounded batches rather than all at once', async () => {
      listings.listed = Array.from(
        { length: 9 },
        (_, index) => `0x${String(index).repeat(40).slice(0, 40)}`,
      );

      let inFlight = 0;
      let peak = 0;
      reader.read = (token, atBlock) => {
        inFlight += 1;
        peak = Math.max(peak, inFlight);
        reader.seen.push({ token, atBlock });
        return Promise.resolve(metadata()).finally(() => {
          inFlight -= 1;
        });
      };

      build(4).onApplicationBootstrap();
      await settle();

      // A public endpoint rate-limits a burst long before it becomes slow, and
      // the transport is configured with `retryCount: 0` — so one 429 under an
      // unbounded `Promise.all` would fail the whole run.
      expect(peak).toBeLessThanOrEqual(4);
      expect(reader.seen).toHaveLength(9);
    });
  });
});
