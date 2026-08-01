import { describe, expect, it } from 'vitest';

import { FakeChainClient, hashOf } from '../../../test/fakes/fake-chain-client';
import type { BlockHeader, ChainClient } from '../../chain/chain-client';
import type { Cursor } from '../cursor/cursor-store';
import { HashChainReorgDetector } from './hash-chain-reorg-detector';
import { InMemoryBlockHeaderStore } from './in-memory-block-header-store';
import type { IndexingOptions } from '../indexing.options';

const CHAIN_ID = 1;

interface Harness {
  readonly detector: HashChainReorgDetector;
  readonly chain: FakeChainClient;
  readonly store: InMemoryBlockHeaderStore;
}

function harness(finalityDepth = 10): Harness {
  const chain = new FakeChainClient({ head: 1_000 });
  const store = new InMemoryBlockHeaderStore();
  const detector = new HashChainReorgDetector(
    { chainId: CHAIN_ID, finalityDepth } as IndexingOptions,
    chain,
    store,
  );

  return { detector, chain, store };
}

/**
 * A chain that forks partway through a sequence of header reads, which is the
 * one thing `FakeChainClient` cannot script on its own — its forks apply the
 * moment they are declared, so every header it then serves is self-consistent.
 */
class ForkingChain implements ChainClient {
  private reads = 0;

  constructor(
    private readonly inner: FakeChainClient,
    private readonly afterReads: number,
    private readonly forkPoint: number,
  ) {}

  getChainId(): Promise<number> {
    return this.inner.getChainId();
  }

  getHeadBlockNumber(): Promise<number> {
    return this.inner.getHeadBlockNumber();
  }

  getBlockHeader(blockNumber: number): Promise<BlockHeader> {
    if (this.reads === this.afterReads) this.inner.forkAbove(this.forkPoint, 'b');
    this.reads += 1;
    return this.inner.getBlockHeader(blockNumber);
  }
}

/** Commits the chain's header for each block, as the chain reads it right now. */
async function commit({ detector, chain }: Harness, ...blockNumbers: number[]): Promise<void> {
  for (const blockNumber of blockNumbers) {
    // Sequential on purpose: each commit prunes relative to its own height, so
    // the order they land in is the retention behaviour under test.
    // oxlint-disable-next-line no-await-in-loop
    await detector.commit(chain.headerAt(blockNumber));
  }
}

function blocks(from: number, to: number): number[] {
  return Array.from({ length: to - from + 1 }, (_, index) => from + index);
}

function cursorAt(lastBlock: number, branch = 'a'): Cursor {
  return { chainId: CHAIN_ID, lastBlock, lastHash: hashOf(branch, lastBlock) };
}

/** Blocks the detector actually read a header for, in order. */
function fetched(chain: FakeChainClient): number[] {
  return chain.calls.flatMap((call) =>
    call.method === 'getBlockHeader' ? [call.blockNumber] : [],
  );
}

async function retained({ store }: Harness): Promise<number[]> {
  return (await store.load(CHAIN_ID)).map((entry) => entry.number);
}

describe('HashChainReorgDetector — finality', () => {
  it('places the safe head one finality depth below the observed head', () => {
    expect(harness(128).detector.safeHead(1_000)).toBe(872);
  });

  it('clamps the safe head rather than going negative on a short chain', () => {
    // A freshly started Anvil is a handful of blocks deep. Without the clamp
    // the loop would compare against a negative bound and dispatch an inverted
    // range.
    expect(harness(128).detector.safeHead(5)).toBe(-1);
  });

  it('treats every block as unsettled when nothing is final', () => {
    expect(harness(128).detector.safeHead(128)).toBe(0);
  });
});

describe('HashChainReorgDetector — inspect', () => {
  it('accepts a block whose parent is the header it retained, without asking the chain', async () => {
    const h = harness();
    await commit(h, 100);

    await expect(h.detector.inspect(h.chain.headerAt(101))).resolves.toEqual({
      type: 'continuous',
    });
    // The parent hash arrives in the header the loop already fetched, so the
    // steady state adds no RPC traffic at all.
    expect(fetched(h.chain)).toEqual([]);
  });

  it('accepts the first block when nothing has been retained', async () => {
    const h = harness();

    await expect(h.detector.inspect(h.chain.headerAt(101))).resolves.toEqual({
      type: 'continuous',
    });
    expect(fetched(h.chain)).toEqual([]);
  });

  it('names the invalid range when the parent hash does not match', async () => {
    const h = harness();
    await commit(h, ...blocks(100, 104));
    h.chain.forkAbove(102, 'b');

    await expect(h.detector.inspect(h.chain.headerAt(105))).resolves.toEqual({
      type: 'reorg',
      firstInvalidBlock: 103,
      lastInvalidBlock: 104,
      lastValidHash: hashOf('a', 102),
    });
  });

  it('handles a one-block fork at the tip', async () => {
    const h = harness();
    await commit(h, ...blocks(100, 104));
    h.chain.forkAbove(103, 'b');

    await expect(h.detector.inspect(h.chain.headerAt(105))).resolves.toEqual({
      type: 'reorg',
      firstInvalidBlock: 104,
      lastInvalidBlock: 104,
      lastValidHash: hashOf('a', 103),
    });
  });

  it('does not re-read the parent it has already disproved', async () => {
    const h = harness();
    await commit(h, ...blocks(100, 104));
    h.chain.forkAbove(102, 'b');

    await h.detector.inspect(h.chain.headerAt(105));

    // The mismatch that opened the walk is proof that 104 is stale; asking the
    // chain about it again would buy nothing.
    expect(fetched(h.chain)).not.toContain(104);
  });

  it('stops descending at the first ancestor the chain still agrees with', async () => {
    const h = harness();
    await commit(h, ...blocks(100, 104));
    h.chain.forkAbove(102, 'b');

    await h.detector.inspect(h.chain.headerAt(105));

    expect(fetched(h.chain)).toEqual([103, 102]);
  });

  it('bounds a fork only as deep as the window it grew after a backfill', async () => {
    const h = harness(128);
    // What a backfill leaves: one anchor, then contiguous blocks at the tip.
    await commit(h, 149, 199, 200, 201);
    h.chain.forkAbove(160, 'b');

    // 150..198 were dispatched as a settled range and never hash-inspected, so
    // there is nothing to check them against. Rewinding to 149 anyway would be
    // safe arithmetic over blocks the detector has no record of — which is the
    // guess it exists not to make.
    await expect(h.detector.inspect(h.chain.headerAt(202))).resolves.toEqual({
      type: 'unrecoverable',
      reason: 'no retained header between 199 and 200 is still canonical',
    });
  });

  it('refuses to walk across a hole rather than treating it as a longer reach', async () => {
    const h = harness(128);
    await commit(h, ...blocks(197, 201));
    // Only a store that lost a row can produce this; the detector's own writes
    // cannot, so the window is holed by reaching past it.
    await h.store.append(CHAIN_ID, h.chain.headerAt(150), 0);
    h.chain.forkAbove(160, 'b');

    // 150 would match the chain and the fork really is above it, so stepping
    // over the hole would give a *safe* answer — and would silently widen the
    // rewind across 47 blocks the window says nothing about.
    await expect(h.detector.inspect(h.chain.headerAt(202))).resolves.toEqual({
      type: 'unrecoverable',
      reason: 'retained headers are not contiguous: nothing at block 196',
    });
  });

  it('stops rather than guessing when the window does not reach the parent block', async () => {
    const h = harness();
    await commit(h, ...blocks(100, 104));
    // A durable window and a durable cursor that drifted apart. The loop cannot
    // cause this — it never skips ahead — so it is corruption, not a fork.
    await h.store.truncate(CHAIN_ID, 102);

    await expect(h.detector.inspect(h.chain.headerAt(105))).resolves.toEqual({
      type: 'unrecoverable',
      reason: 'nothing retained for block 104, so block 105 has no ancestry to check',
    });
  });

  it('counts a block it committed but never cursored as invalid', async () => {
    const h = harness();
    // The loop commits a header before saving the cursor. A rejected save
    // leaves 101 retained with the cursor still at 100, so the loop re-inspects
    // 101 — and 101's writes are already out there.
    await commit(h, ...blocks(95, 101));
    h.chain.forkAbove(97, 'b');

    await expect(h.detector.inspect(h.chain.headerAt(101))).resolves.toEqual({
      type: 'reorg',
      firstInvalidBlock: 98,
      // 101, not 100: reporting the block the caller implies would orphan
      // everything the processors derived from it.
      lastInvalidBlock: 101,
      lastValidHash: hashOf('a', 97),
    });
  });

  it('tolerates re-inspecting a block it has already committed', async () => {
    const h = harness();
    await commit(h, 100, 101);

    // The precondition is soft: after a failed cursor save the loop replays a
    // block the detector already holds. Treating that as a violation would turn
    // a benign retry into a crash loop.
    await expect(h.detector.inspect(h.chain.headerAt(101))).resolves.toEqual({
      type: 'continuous',
    });
  });

  it('refuses to guess when nothing it retained is still canonical', async () => {
    const h = harness();
    await commit(h, ...blocks(100, 104));
    h.chain.forkAbove(99, 'b');

    await expect(h.detector.inspect(h.chain.headerAt(105))).resolves.toEqual({
      type: 'unrecoverable',
      reason: 'no retained header between 100 and 103 is still canonical',
    });
  });

  it('surfaces an RPC failure rather than mistaking it for a fork', async () => {
    const h = harness();
    await commit(h, ...blocks(100, 104));
    h.chain.forkAbove(102, 'b').failNext(1);

    await expect(h.detector.inspect(h.chain.headerAt(105))).rejects.toThrow('rpc unavailable');
    // The loop turns a rejection into a retry, so the window must come through
    // the failed walk untouched.
    await expect(retained(h)).resolves.toEqual(blocks(100, 104));
  });
});

describe('HashChainReorgDetector — bootstrap', () => {
  it('starts from a clean window when there is no stored cursor', async () => {
    const h = harness();
    await commit(h, 100, 101);

    await expect(h.detector.bootstrap(null)).resolves.toEqual({ type: 'continuous' });
    // Nothing has been indexed, so nothing retained can belong to this run.
    await expect(retained(h)).resolves.toEqual([]);
  });

  it('settles a canonical cursor with one header read and no walk', async () => {
    const h = harness();

    await expect(h.detector.bootstrap(cursorAt(500))).resolves.toEqual({ type: 'continuous' });
    // A hash commits to its whole ancestry, so a matching one at 500 proves 499
    // and everything below it too. Reading them would answer nothing.
    expect(fetched(h.chain)).toEqual([500]);
  });

  it('rebuilds the window under a resume point near the tip', async () => {
    // Head 1000, depth 10, so the safe head is 990 and the cursor is above it.
    const h = harness(10);

    await h.detector.bootstrap(cursorAt(998));

    // Down to the safe head, which is as deep as a fork can reach — every one
    // of them reached by following parentHash from a block the cursor proved.
    await expect(retained(h)).resolves.toEqual(blocks(990, 998));
  });

  it('places a fork on the very next block after a resume', async () => {
    const h = harness(10);
    await h.detector.bootstrap(cursorAt(998));

    h.chain.forkAbove(993, 'b');

    // This is what the refill buys. With only the seed retained, the walk would
    // have nothing below 998 to compare and would answer unrecoverable — a
    // restart would blind the indexer for its next ~10 blocks.
    await expect(h.detector.inspect(h.chain.headerAt(999))).resolves.toEqual({
      type: 'reorg',
      firstInvalidBlock: 994,
      lastInvalidBlock: 998,
      lastValidHash: hashOf('a', 993),
    });
  });

  it('does not rebuild the window when resuming inside the settled range', async () => {
    const h = harness(10);

    // Cursor at 500 against a head of 1000: mid-backfill, far below the safe
    // head. The loop is about to dispatch wide ranges and re-anchor the window
    // on each one, so a refill would be calls spent on soon-discarded headers.
    await h.detector.bootstrap(cursorAt(500));

    await expect(retained(h)).resolves.toEqual([500]);
    expect(fetched(h.chain)).toEqual([500]);
  });

  it('stops rebuilding where the chain moved underneath it', async () => {
    const chain = new FakeChainClient({ head: 1_000 });
    const store = new InMemoryBlockHeaderStore();
    const detector = new HashChainReorgDetector(
      { chainId: CHAIN_ID, finalityDepth: 10 } as IndexingOptions,
      // Reads run 998, 997, 996, 995 — this forks in time for the last of them,
      // so 995 comes back on a branch 996 never named as its parent.
      new ForkingChain(chain, 3, 994),
      store,
    );

    await detector.bootstrap(cursorAt(998));

    // 995 now reads as branch b, which is not what 996 names as its parent.
    // Splicing it in would leave a window that looks like a chain and is not,
    // and the walk would later stop at a block we never processed.
    expect((await store.load(CHAIN_ID)).map((entry) => entry.number)).toEqual([996, 997, 998]);
  });

  it('seeds the window from the verified cursor header', async () => {
    const h = harness();
    await h.detector.bootstrap(cursorAt(500));
    // The chain forks after the resume check but before the next block.
    h.chain.forkAbove(499, 'b');

    // Without the seed the window would still be empty here, block 501 would
    // pass as continuous, and the losing branch would become the anchor
    // everything after it is checked against.
    await expect(h.detector.inspect(h.chain.headerAt(501))).resolves.toEqual({
      type: 'unrecoverable',
      reason: 'nothing retained at or below block 499 to compare against',
    });
  });

  it('unwinds a fork that happened while the process was down', async () => {
    const h = harness();
    await commit(h, ...blocks(494, 500));
    h.chain.forkAbove(496, 'b');

    await expect(h.detector.bootstrap(cursorAt(500))).resolves.toEqual({
      type: 'reorg',
      firstInvalidBlock: 497,
      lastInvalidBlock: 500,
      lastValidHash: hashOf('a', 496),
    });
  });

  it('refuses to resume when the window did not survive the restart', async () => {
    const h = harness();
    h.chain.forkAbove(499, 'b');

    // One hash cannot locate a fork point. This is what an in-memory window
    // costs a durable cursor, and it is why the two want to land together.
    await expect(h.detector.bootstrap(cursorAt(500))).resolves.toEqual({
      type: 'unrecoverable',
      reason: 'nothing retained at or below block 499 to compare against',
    });
  });

  it('reports the same fork twice when the loop could not apply it', async () => {
    const h = harness();
    await commit(h, ...blocks(494, 500));
    h.chain.forkAbove(496, 'b');

    const first = await h.detector.bootstrap(cursorAt(500));

    // A processor that answers `retry` leaves the whole rewind unapplied and
    // the loop bootstraps again from scratch.
    await expect(h.detector.bootstrap(cursorAt(500))).resolves.toEqual(first);
  });

  it('re-reports the whole range after a rewind whose cursor save failed', async () => {
    const h = harness();
    await commit(h, ...blocks(494, 500));
    h.chain.forkAbove(496, 'b');

    await h.detector.bootstrap(cursorAt(500));
    await h.detector.rewindTo(496);

    // The window is now three blocks below the cursor, which still points into
    // the abandoned branch. Reporting the window's top as the last invalid
    // block would hand the loop onReorg(497, 496) — an inverted range.
    await expect(h.detector.bootstrap(cursorAt(500))).resolves.toEqual({
      type: 'reorg',
      firstInvalidBlock: 497,
      lastInvalidBlock: 500,
      lastValidHash: hashOf('a', 496),
    });
  });
});

describe('HashChainReorgDetector — retention', () => {
  it('keeps the oldest header the deepest recoverable fork needs', async () => {
    const h = harness(10);

    await commit(h, ...blocks(100, 110));

    // finalityDepth + 1 headers, and the extra one is not slack: the deepest
    // fork the loop can hand over begins at safeHead + 1, and placing it means
    // matching its parent at exactly safeHead.
    await expect(retained(h)).resolves.toEqual(blocks(100, 110));
  });

  it('drops the headers a further commit pushes out of the window', async () => {
    const h = harness(10);

    await commit(h, ...blocks(100, 111));

    await expect(retained(h)).resolves.toEqual(blocks(101, 111));
  });

  it('keeps only the anchor when a settled range jumps the window forward', async () => {
    const h = harness(128);

    // Every range top during a backfill lands clear of the last one. Keeping
    // both would leave a hole between them, and a run on the far side of a hole
    // cannot be walked back through — so the earlier anchor is not evidence of
    // anything and goes.
    await commit(h, 149, 199);

    await expect(retained(h)).resolves.toEqual([199]);
  });

  it('grows contiguously from the anchor once the loop reaches the tip', async () => {
    const h = harness(128);

    await commit(h, 149, 199, 200, 201, 202);

    // The anchor is what the first inspected block is checked against, and
    // everything above it arrives one block at a time.
    await expect(retained(h)).resolves.toEqual([199, 200, 201, 202]);
  });

  it('discards what a rewind invalidates, keeping the block rewound onto', async () => {
    const h = harness();
    await commit(h, ...blocks(100, 105));

    await h.detector.rewindTo(102);

    // 102 is the ancestor the loop is resuming from, so the next block indexed
    // is checked against it.
    await expect(retained(h)).resolves.toEqual([100, 101, 102]);
  });

  it('clears the window when rewound below every header it holds', async () => {
    const h = harness();
    await commit(h, 100);

    await h.detector.rewindTo(99);

    await expect(retained(h)).resolves.toEqual([]);
  });

  it('replaces a height re-committed on another branch', async () => {
    const h = harness();
    await commit(h, 100);
    h.chain.forkAbove(99, 'b');

    await commit(h, 100);

    // Two answers for one height would leave the walk unable to say which
    // branch it was on.
    await expect(h.store.load(CHAIN_ID)).resolves.toEqual([h.chain.headerAt(100)]);
  });
});
