import type {
  Address,
  BlockHeader,
  ChainClient,
  Erc20MetadataReader,
  TokenMetadata,
} from '@packages/indexing';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { TokenListings } from '../store/token-listing-source';
import type { TokenLabel, TokenMetadataRow } from '../store/token-metadata';
import type { TokenMetadataStore } from '../store/token-metadata-store';
import { TokenEnrichmentProcessor } from './token-enrichment.processor';

const CHAIN_ID = 1;
const RETRY_MS = 60_000;
const HEAD = 25_652_782;

const USDC = '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48';
const WETH = '0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2';

const NEVER_ABORTED = new AbortController().signal;

function metadata(over: Partial<TokenMetadata> = {}): TokenMetadata {
  return { symbol: 'USDC', name: 'USD Coin', decimals: 6, failures: [], ...over };
}

/**
 * Drains the microtask queue.
 *
 * The whole point of this processor is that nothing hands the caller a promise
 * to await, so a test cannot wait on the work the way production does not
 * either — it has to let the queue run down instead. `setImmediate` yields past
 * every pending microtask chain in one go.
 */
const settle = (): Promise<void> => new Promise((resolve) => setImmediate(resolve));

class FakeListings implements TokenListings {
  listed: Address[] = [];
  addedByRange: Address[] = [];
  /** Which shape each call took, in order — the whole set, or one range. */
  readonly calls: ('all' | 'addedIn')[] = [];
  readonly ranges: { from: number; to: number }[] = [];

  all(): Promise<readonly Address[]> {
    this.calls.push('all');
    return Promise.resolve(this.listed);
  }

  addedIn(_chainId: number, from: number, to: number): Promise<readonly Address[]> {
    this.calls.push('addedIn');
    this.ranges.push({ from, to });
    return Promise.resolve(this.addedByRange);
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
let store: FakeStore;
let reader: FakeReader;
let chain: FakeChain;
let processor: TokenEnrichmentProcessor;

function build(concurrency = 4): TokenEnrichmentProcessor {
  return new TokenEnrichmentProcessor(
    { chainId: CHAIN_ID, retryDelayMs: RETRY_MS, concurrency },
    listings,
    store,
    reader,
    chain,
  );
}

describe('TokenEnrichmentProcessor', () => {
  beforeEach(() => {
    // Only `Date`, and always restored. Two tests move the clock to step over
    // the back-off; left unscoped that shift leaks into whatever else is
    // running in this process, and a sibling suite asserting on `Date.now()`
    // fails somewhere else entirely. `setImmediate` stays real because
    // `settle` depends on it.
    vi.useFakeTimers({ toFake: ['Date'] });
    listings = new FakeListings();
    store = new FakeStore();
    reader = new FakeReader();
    chain = new FakeChain();
    processor = build();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  describe('what it does to the loop', () => {
    it('hands the loop an outcome, not a promise to wait on', () => {
      listings.listed = [USDC];

      const outcome = processor.onBlockRange(100, 200, NEVER_ABORTED);

      // `BlockProcessor` allows either, and returning the value rather than a
      // promise is what makes it impossible to add an `await` in here without
      // noticing. The indexer staying in step with the chain is the job; a
      // token symbol is not, and must never be ahead of it in the queue.
      expect(outcome).toEqual({ status: 'ok' });
      expect(outcome).not.toBeInstanceOf(Promise);
      expect(store.rows.size).toBe(0);
    });

    it('returns while an ERC-20 read is still outstanding', async () => {
      listings.listed = [USDC];
      let release!: () => void;
      reader.gate = new Promise<void>((resolve) => {
        release = resolve;
      });

      processor.onBlockRange(100, 200, NEVER_ABORTED);
      await settle();

      // The read is in flight and the dispatch is long since over. Awaiting it
      // would hold the Spoke and Hub ledgers behind a third-party contract.
      expect(reader.seen).toHaveLength(1);
      expect(store.rows.size).toBe(0);

      release();
      await settle();
      expect(store.rows.has(USDC)).toBe(true);
    });

    it('starts nothing while a run is already in flight', async () => {
      listings.listed = [USDC];
      let release!: () => void;
      reader.gate = new Promise<void>((resolve) => {
        release = resolve;
      });

      processor.onBlockRange(100, 200, NEVER_ABORTED);
      await settle();
      processor.onBlockRange(200, 300, NEVER_ABORTED);
      processor.onBlockRange(300, 400, NEVER_ABORTED);
      await settle();

      // At head a dispatch lands every few seconds; without this each one would
      // pile another fan-out onto the same provider.
      expect(listings.calls).toHaveLength(1);

      release();
      await settle();
    });

    it('returns ok when the reader throws', async () => {
      listings.listed = [USDC];
      reader.throws = new Error('provider exploded');

      expect(processor.onBlockRange(100, 200, NEVER_ABORTED)).toEqual({ status: 'ok' });
      await settle();
      expect(store.rows.size).toBe(0);
    });

    it('returns ok when the chain head cannot be read', async () => {
      listings.listed = [USDC];
      chain.throws = new Error('no providers');

      expect(processor.onBlockRange(100, 200, NEVER_ABORTED)).toEqual({ status: 'ok' });
      await settle();
    });

    it('starts nothing once shutdown has been signalled', () => {
      listings.listed = [USDC];

      processor.onBlockRange(100, 200, AbortSignal.abort());

      expect(listings.calls).toEqual([]);
    });

    it('does nothing on a reorg, because a fork does not unmint an ERC-20', () => {
      expect(processor.onReorg()).toEqual({ status: 'ok' });
    });
  });

  describe('what wakes it up', () => {
    it('asks for the whole listing set the first time, because nothing has been checked', async () => {
      listings.listed = [USDC];

      processor.onBlockRange(25_000_000, 25_001_000, NEVER_ABORTED);
      await settle();

      // Every `AddAsset` on mainnet fired at block 24,722,784, far behind any
      // live cursor. A trigger-only start would see nothing and enrich nothing.
      expect(listings.calls).toEqual(['all']);
      expect(store.rows.has(USDC)).toBe(true);
    });

    it('asks only about the range once it has caught up', async () => {
      listings.listed = [USDC];
      processor.onBlockRange(100, 200, NEVER_ABORTED);
      await settle();

      processor.onBlockRange(200, 300, NEVER_ABORTED);
      await settle();

      expect(listings.calls).toEqual(['all', 'addedIn']);
    });

    it('stops at the range query when no token was listed in it', async () => {
      listings.listed = [USDC];
      processor.onBlockRange(100, 200, NEVER_ABORTED);
      await settle();
      const labelsAfterCatchUp = store.labelCalls;

      processor.onBlockRange(200, 300, NEVER_ABORTED);
      processor.onBlockRange(300, 400, NEVER_ABORTED);
      await settle();

      // The point of the whole arrangement. `AddAsset` is the only event that
      // can change the answer, so a range without one cannot have work — and
      // every dispatch after genesis is such a range. No Postgres, no chain.
      expect(store.labelCalls).toBe(labelsAfterCatchUp);
      expect(reader.seen).toHaveLength(1);
    });

    it('enriches a token the moment its listing lands in a range', async () => {
      listings.listed = [USDC];
      processor.onBlockRange(100, 200, NEVER_ABORTED);
      await settle();

      listings.addedByRange = [WETH];
      processor.onBlockRange(25_500_000, 25_501_000, NEVER_ABORTED);
      await settle();

      expect(store.rows.has(WETH)).toBe(true);
    });

    it('overlaps the previous range, so a late write is not missed forever', async () => {
      listings.listed = [];
      processor.onBlockRange(100, 200, NEVER_ABORTED);
      await settle();

      processor.onBlockRange(1_000, 1_999, NEVER_ABORTED);
      await settle();

      // Today the Hub processor has always written first — dispatch is ordered
      // and fail-fast. `dispatch.ts` plans to drop that, and a missed range
      // never comes back, so the trigger reaches one range further back than
      // it strictly needs to.
      expect(listings.ranges.at(-1)).toEqual({ from: 0, to: 1_999 });

      processor.onBlockRange(10_000, 10_999, NEVER_ABORTED);
      await settle();
      expect(listings.ranges.at(-1)).toEqual({ from: 9_000, to: 10_999 });
    });

    it('goes back to the whole set after a failure, not to the range', async () => {
      // Deliberately after a *successful* run, which is the only way to tell
      // the two apart: on a cold start the flag is already set, so a mutation
      // that never sets it would still look correct there.
      listings.listed = [USDC];
      processor.onBlockRange(100, 200, NEVER_ABORTED);
      await settle();
      expect(listings.calls).toEqual(['all']);

      // A listing lands, and the token cannot be reached.
      listings.addedByRange = [WETH];
      reader.answers.set(WETH, {
        symbol: null,
        name: null,
        decimals: null,
        failures: ['symbol: HttpRequestError'],
      });
      processor.onBlockRange(200, 300, NEVER_ABORTED);
      await settle();
      expect(listings.calls).toEqual(['all', 'addedIn']);
      expect(store.rows.has(WETH)).toBe(false);

      // The retry must not ask about a range — WETH's listing is behind it now,
      // so the range would return nothing and the gap would never close.
      vi.setSystemTime(Date.now() + RETRY_MS);
      listings.listed = [USDC, WETH];
      listings.addedByRange = [];
      reader.answers.delete(WETH);
      processor.onBlockRange(300, 400, NEVER_ABORTED);
      await settle();

      expect(listings.calls).toEqual(['all', 'addedIn', 'all']);
      expect(store.rows.has(WETH)).toBe(true);
    });
  });

  describe('backing off', () => {
    it('waits before trying again after a gap is left open', async () => {
      listings.listed = [USDC];
      chain.throws = new Error('no providers');

      processor.onBlockRange(100, 200, NEVER_ABORTED);
      await settle();
      expect(listings.calls).toHaveLength(1);

      processor.onBlockRange(200, 300, NEVER_ABORTED);
      await settle();

      // A dead provider is retried on a timer rather than on every block.
      expect(listings.calls).toHaveLength(1);
    });

    it('tries again once the delay has passed', async () => {
      listings.listed = [USDC];
      chain.throws = new Error('no providers');

      processor.onBlockRange(100, 200, NEVER_ABORTED);
      await settle();

      vi.setSystemTime(Date.now() + RETRY_MS);
      chain.throws = null;
      processor.onBlockRange(200, 300, NEVER_ABORTED);
      await settle();

      expect(store.rows.has(USDC)).toBe(true);
    });

    it('does not back off when there was simply nothing to do', async () => {
      listings.listed = [USDC];
      processor.onBlockRange(100, 200, NEVER_ABORTED);
      await settle();

      listings.addedByRange = [WETH];
      processor.onBlockRange(200, 300, NEVER_ABORTED);
      await settle();

      // A successful run imposes no delay, so a new listing is picked up on the
      // very next dispatch rather than waiting out a timer it did not earn.
      expect(store.rows.has(WETH)).toBe(true);
    });
  });

  describe('what it stores', () => {
    it('enriches every listed token the store does not have', async () => {
      listings.listed = [USDC, WETH];

      processor.onBlockRange(100, 200, NEVER_ABORTED);
      await settle();

      expect([...store.rows.keys()].toSorted()).toEqual([USDC, WETH].toSorted());
      expect(store.rows.get(USDC)?.fetchedAtBlock).toBe(HEAD);
    });

    it('skips a token it already has', async () => {
      listings.listed = [USDC];
      listings.addedByRange = [USDC];
      processor.onBlockRange(100, 200, NEVER_ABORTED);
      await settle();

      processor.onBlockRange(200, 300, NEVER_ABORTED);
      await settle();

      expect(reader.seen).toHaveLength(1);
    });

    it('asks the store nothing when the Hub has listed nothing', async () => {
      processor.onBlockRange(100, 200, NEVER_ABORTED);
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

      processor.onBlockRange(100, 200, NEVER_ABORTED);
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

      processor.onBlockRange(100, 200, NEVER_ABORTED);
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

      processor.onBlockRange(100, 200, NEVER_ABORTED);
      await settle();

      expect(store.rows.get(USDC)).toMatchObject({ symbol: 'USDC', tokenDecimals: null });
    });

    it('reads at the chain head, not at the range it was handed', async () => {
      listings.listed = [USDC];

      processor.onBlockRange(24_000_000, 24_001_000, NEVER_ABORTED);
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

      processor.onBlockRange(100, 200, shutdown.signal);
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

      build(4).onBlockRange(100, 200, NEVER_ABORTED);
      await settle();

      // A public endpoint rate-limits a burst long before it becomes slow, and
      // the transport is configured with `retryCount: 0` — so one 429 under an
      // unbounded `Promise.all` would fail the whole run.
      expect(peak).toBeLessThanOrEqual(4);
      expect(reader.seen).toHaveLength(9);
    });
  });
});
