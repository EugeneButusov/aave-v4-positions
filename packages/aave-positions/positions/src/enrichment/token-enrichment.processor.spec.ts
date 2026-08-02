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
const SWEEP_MS = 300_000;
const HEAD = 25_652_782;

const USDC = '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48';
const WETH = '0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2';

const NEVER_ABORTED = new AbortController().signal;

function metadata(over: Partial<TokenMetadata> = {}): TokenMetadata {
  return { symbol: 'USDC', name: 'USD Coin', decimals: 6, failures: [], ...over };
}

class FakeListings implements TokenListings {
  everListed: Address[] = [];
  addedByRange: Address[] = [];
  readonly calls: string[] = [];

  all(): Promise<readonly Address[]> {
    this.calls.push('all');
    return Promise.resolve(this.everListed);
  }

  addedIn(): Promise<readonly Address[]> {
    this.calls.push('addedIn');
    return Promise.resolve(this.addedByRange);
  }
}

class FakeStore implements TokenMetadataStore {
  readonly rows = new Map<Address, TokenMetadataRow>();

  labels(): Promise<ReadonlyMap<Address, TokenLabel>> {
    return Promise.resolve(new Map());
  }

  known(): Promise<ReadonlySet<Address>> {
    return Promise.resolve(new Set(this.rows.keys()));
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

  read(token: Address, atBlock: bigint): Promise<TokenMetadata> {
    this.seen.push({ token, atBlock });
    if (this.throws) return Promise.reject(this.throws);
    return Promise.resolve(this.answers.get(token) ?? metadata());
  }
}

class FakeChain implements ChainClient {
  head = HEAD;
  throws: Error | null = null;

  getChainId(): Promise<number> {
    return Promise.resolve(CHAIN_ID);
  }

  getHeadBlockNumber(): Promise<number> {
    if (this.throws) return Promise.reject(this.throws);
    return Promise.resolve(this.head);
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
    { chainId: CHAIN_ID, sweepIntervalMs: SWEEP_MS, concurrency },
    listings,
    store,
    reader,
    chain,
  );
}

describe('TokenEnrichmentProcessor', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    listings = new FakeListings();
    store = new FakeStore();
    reader = new FakeReader();
    chain = new FakeChain();
    processor = build();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  describe('the sweep', () => {
    it('runs on the first dispatch, before any interval has passed', async () => {
      listings.everListed = [USDC];

      await processor.onBlockRange(100, 200, NEVER_ABORTED);

      // The bootstrap case, and the reason the fast path alone is not enough:
      // every AddAsset on mainnet fired at block 24,722,784, far behind any
      // live cursor, so a range-scoped query would find nothing on a fresh
      // start.
      expect(listings.calls).toEqual(['all']);
      expect(store.rows.get(USDC)?.symbol).toBe('USDC');
    });

    it('does not sweep again inside the interval', async () => {
      listings.everListed = [USDC];
      await processor.onBlockRange(100, 200, NEVER_ABORTED);

      vi.advanceTimersByTime(SWEEP_MS - 1);
      await processor.onBlockRange(200, 300, NEVER_ABORTED);

      expect(listings.calls).toEqual(['all', 'addedIn']);
    });

    it('sweeps again once the interval has passed', async () => {
      await processor.onBlockRange(100, 200, NEVER_ABORTED);

      vi.advanceTimersByTime(SWEEP_MS);
      await processor.onBlockRange(200, 300, NEVER_ABORTED);

      expect(listings.calls).toEqual(['all', 'all']);
    });

    it('picks up a listing the fast path missed', async () => {
      // The property the whole two-mechanism shape rests on. The fast path
      // depends on running after the Hub processor, and `dispatch.ts` plans to
      // drop that guarantee — so a miss has to degrade to "one sweep late"
      // rather than "silently never".
      await processor.onBlockRange(100, 200, NEVER_ABORTED);
      listings.addedByRange = [];
      listings.everListed = [WETH];

      await processor.onBlockRange(200, 300, NEVER_ABORTED);
      expect(store.rows.has(WETH)).toBe(false);

      vi.advanceTimersByTime(SWEEP_MS);
      await processor.onBlockRange(300, 400, NEVER_ABORTED);
      expect(store.rows.has(WETH)).toBe(true);
    });
  });

  describe('the fast path', () => {
    it('enriches a token listed in this very range', async () => {
      await processor.onBlockRange(100, 200, NEVER_ABORTED);
      listings.addedByRange = [WETH];

      await processor.onBlockRange(200, 300, NEVER_ABORTED);

      expect(store.rows.has(WETH)).toBe(true);
    });

    it('asks the store nothing when the range listed nothing', async () => {
      await processor.onBlockRange(100, 200, NEVER_ABORTED);
      const known = vi.spyOn(store, 'known');

      await processor.onBlockRange(200, 300, NEVER_ABORTED);

      // Which is every range after genesis. The seek is cheap; a second query
      // for a set that is always empty would not be.
      expect(known).not.toHaveBeenCalled();
    });
  });

  describe('what it stores', () => {
    it('skips a token it already has', async () => {
      listings.everListed = [USDC];
      await processor.onBlockRange(100, 200, NEVER_ABORTED);

      vi.advanceTimersByTime(SWEEP_MS);
      await processor.onBlockRange(200, 300, NEVER_ABORTED);

      expect(reader.seen).toHaveLength(1);
    });

    it('writes a row for a token that has no symbol at all', async () => {
      listings.everListed = [USDC];
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

      await processor.onBlockRange(100, 200, NEVER_ABORTED);

      // The row existing is what records that the question was put. Without it
      // a conformant token with no optional metadata is re-read on every sweep,
      // forever.
      expect(store.rows.get(USDC)).toMatchObject({ symbol: null, name: null });
    });

    it('leaves the gap open when the node could not be reached', async () => {
      listings.everListed = [USDC];
      reader.answers.set(USDC, {
        symbol: null,
        name: null,
        decimals: null,
        failures: ['symbol: HttpRequestError', 'name: HttpRequestError', 'decimals: TimeoutError'],
      });

      await processor.onBlockRange(100, 200, NEVER_ABORTED);

      // The other half of the rule above, and the trap it exists to avoid: a
      // timeout written as a null closes the gap on a label nobody will ever
      // revisit.
      expect(store.rows.has(USDC)).toBe(false);
    });

    it('treats a value it rejected itself as the token having answered', async () => {
      listings.everListed = [USDC];
      reader.answers.set(
        USDC,
        metadata({ decimals: null, failures: ['decimals: out of range (999)'] }),
      );

      await processor.onBlockRange(100, 200, NEVER_ABORTED);

      expect(store.rows.get(USDC)).toMatchObject({ symbol: 'USDC', tokenDecimals: null });
    });

    it('reads at the chain head, not at the range it was handed', async () => {
      listings.everListed = [USDC];

      await processor.onBlockRange(24_000_000, 24_001_000, NEVER_ABORTED);

      // During a backfill the range is historical and a full node cannot serve
      // state there — every call would fail, and fail in a way indistinguishable
      // from a token with no symbol. Metadata is immutable, so the head is both
      // correct and answerable.
      expect(reader.seen[0]?.atBlock).toBe(BigInt(HEAD));
      expect(store.rows.get(USDC)?.fetchedAtBlock).toBe(HEAD);
    });
  });

  describe('what it refuses to do to the loop', () => {
    it('returns ok when the reader throws', async () => {
      listings.everListed = [USDC];
      reader.throws = new Error('provider exploded');

      // A third-party token contract must never stall Aave ingestion, and a
      // `retry` would do exactly that.
      await expect(processor.onBlockRange(100, 200, NEVER_ABORTED)).resolves.toEqual({
        status: 'ok',
      });
      expect(store.rows.size).toBe(0);
    });

    it('returns ok when the chain head cannot be read', async () => {
      listings.everListed = [USDC];
      chain.throws = new Error('no providers');

      await expect(processor.onBlockRange(100, 200, NEVER_ABORTED)).resolves.toEqual({
        status: 'ok',
      });
    });

    it('leaves the gap open after a failure, so the next sweep retries', async () => {
      listings.everListed = [USDC];
      chain.throws = new Error('no providers');
      await processor.onBlockRange(100, 200, NEVER_ABORTED);

      chain.throws = null;
      vi.advanceTimersByTime(SWEEP_MS);
      await processor.onBlockRange(200, 300, NEVER_ABORTED);

      // Swallowing the error is only safe because discovery is gap-driven and
      // idempotent. This is the half that makes it so.
      expect(store.rows.has(USDC)).toBe(true);
    });

    it('writes nothing once shutdown has been signalled', async () => {
      listings.everListed = [USDC];
      const aborted = AbortSignal.abort();

      await processor.onBlockRange(100, 200, aborted);

      expect(store.rows.size).toBe(0);
    });

    it('does nothing on a reorg, because a fork does not unmint an ERC-20', () => {
      expect(processor.onReorg()).toEqual({ status: 'ok' });
    });
  });

  describe('concurrency', () => {
    it('reads in bounded batches rather than all at once', async () => {
      listings.everListed = Array.from(
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

      await build(4).onBlockRange(100, 200, NEVER_ABORTED);

      // A public endpoint rate-limits a burst long before it becomes slow, and
      // the transport is configured with `retryCount: 0` — so one 429 under an
      // unbounded `Promise.all` would fail the whole sweep.
      expect(peak).toBeLessThanOrEqual(4);
      expect(reader.seen).toHaveLength(9);
    });
  });
});
