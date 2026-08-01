import { describe, expect, it } from 'vitest';

import { FakeChainClient, hashOf } from '../../../test/fakes/fake-chain-client';
import { ForkingChain } from '../../../test/fakes/forking-chain';
import {
  CHAIN_ID,
  blocks,
  commit,
  cursorAt,
  harness,
  retained,
} from '../../../test/fakes/reorg-harness';
import { ShufflingBlockHeaderStore } from '../../../test/fakes/shuffling-block-header-store';
import { HashChainReorgDetector } from './hash-chain-reorg-detector';
import { InMemoryBlockHeaderStore } from './in-memory-block-header-store';
import type { IndexingOptions } from '../indexing.options';

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
});

describe('HashChainReorgDetector — inspect', () => {
  it('accepts a block whose parent is the header it retained', async () => {
    const h = harness();
    await commit(h, 100);

    await expect(h.detector.inspect(h.chain.headerAt(101))).resolves.toEqual({
      type: 'continuous',
    });
  });

  it('accepts the first block when nothing has been retained', async () => {
    const h = harness();

    await expect(h.detector.inspect(h.chain.headerAt(101))).resolves.toEqual({
      type: 'continuous',
    });
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

  it('catches a swap at a height it committed but never cursored', async () => {
    const h = harness();
    await commit(h, ...blocks(495, 500));
    // Only block 500 is replaced, so 499 — and therefore 500's parent link —
    // still checks out. The cursor never reached 500, so the loop replays it.
    h.chain.forkAbove(499, 'b');

    // Waving this through would fold the replacement on top of a block whose
    // events nothing ever told the processors to discard.
    await expect(h.detector.inspect(h.chain.headerAt(500))).resolves.toEqual({
      type: 'reorg',
      firstInvalidBlock: 500,
      lastInvalidBlock: 500,
      lastValidHash: hashOf('a', 499),
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

  it('does not depend on the order the store returns the window in', async () => {
    const chain = new FakeChainClient({ head: 1_000 });
    const detector = new HashChainReorgDetector(
      { chainId: CHAIN_ID, finalityDepth: 10 } as IndexingOptions,
      chain,
      new ShufflingBlockHeaderStore(),
    );
    await commit({ detector, chain }, ...blocks(100, 104));
    chain.forkAbove(102, 'b');

    // Nearly everything reads the window positionally — the last entry bounds
    // the invalid range, the first bounds a failed walk, and the walk descends.
    // Taken in the wrong order this under-reports, which loses writes.
    await expect(detector.inspect(chain.headerAt(105))).resolves.toEqual({
      type: 'reorg',
      firstInvalidBlock: 103,
      lastInvalidBlock: 104,
      lastValidHash: hashOf('a', 102),
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

  it('settles a cursor the chain still agrees with', async () => {
    const h = harness();

    // A hash commits to its whole ancestry, so a match at 500 settles 499 and
    // everything below it too.
    await expect(h.detector.bootstrap(cursorAt(500))).resolves.toEqual({ type: 'continuous' });
  });

  it('rebuilds the window under a resume point near the tip', async () => {
    // Head 1000, depth 10, so the safe head is 990 and the cursor is above it.
    const h = harness(10);

    await h.detector.bootstrap(cursorAt(998));

    // A full retention window, every header of it reached by following
    // parentHash down from a block the cursor proved was ours.
    await expect(retained(h)).resolves.toEqual(blocks(988, 998));
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

  it('catches a fork that replaced only the resume point itself', async () => {
    const h = harness();
    await commit(h, ...blocks(494, 500));
    // Only 500 is replaced. 499 is untouched, so 500's replacement names the
    // very same parent — the link between them checks out on both branches.
    h.chain.forkAbove(499, 'b');

    // Which is why the comparison is against the hash we recorded for 500 and
    // not against its parent link: a check that asked whether the canonical 500
    // descends from our 499 would answer yes and fold the losing branch's
    // events under its replacement.
    await expect(h.detector.bootstrap(cursorAt(500))).resolves.toEqual({
      type: 'reorg',
      firstInvalidBlock: 500,
      lastInvalidBlock: 500,
      lastValidHash: hashOf('a', 499),
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
});

/**
 * The loop rewinds only after the cursor is durable, so an unfinished unwind
 * leaves the window holding the run above a cursor that already moved. That
 * disagreement is the only record that a reorg is owed.
 */
describe('HashChainReorgDetector — an owed reorg survives the process', () => {
  it('reports the same fork again when nothing was applied', async () => {
    const h = harness();
    await commit(h, ...blocks(494, 500));
    h.chain.forkAbove(496, 'b');
    const reported = await h.detector.inspect(h.chain.headerAt(501));

    // Died before the discard was even dispatched. The cursor was never moved,
    // so it still names 500 on the branch that lost.
    await expect(h.detector.bootstrap(cursorAt(500))).resolves.toEqual(reported);
  });

  it('reports the same fork again when the cursor landed but the rewind did not', async () => {
    const h = harness();
    await commit(h, ...blocks(494, 500));
    h.chain.forkAbove(496, 'b');
    const reported = await h.detector.inspect(h.chain.headerAt(501));

    // The gap the loop actually leaves: it saves the cursor before rewinding,
    // so a crash here has the cursor already naming 496 while the window still
    // holds 497..500 from the branch that lost.
    await expect(h.detector.bootstrap(cursorAt(496))).resolves.toEqual(reported);
  });

  it('stops reporting it once the rewound cursor has been saved', async () => {
    const h = harness();
    await commit(h, ...blocks(494, 500));
    h.chain.forkAbove(496, 'b');
    await h.detector.inspect(h.chain.headerAt(501));
    await h.detector.rewindTo(496);

    // The cursor is the single durable commit point: once it names 496, the
    // unwinding is done and the loop may go forward.
    await expect(h.detector.bootstrap(cursorAt(496))).resolves.toEqual({ type: 'continuous' });
  });

  it('vets the block it committed, not just the one the cursor names', async () => {
    const h = harness();
    await commit(h, ...blocks(495, 500));
    // The cursor save for 500 was the write that failed, so it still names 499.
    // Then only 500 is replaced, leaving 499 canonical.
    h.chain.forkAbove(499, 'b');

    // Checking the cursor alone would answer continuous here — 499 is fine —
    // and 500's events would stay folded under its replacement.
    await expect(h.detector.bootstrap(cursorAt(499))).resolves.toEqual({
      type: 'reorg',
      firstInvalidBlock: 500,
      lastInvalidBlock: 500,
      lastValidHash: hashOf('a', 499),
    });
  });

  it('cannot tell an unapplied fork from one that happened while it was down', async () => {
    const h = harness();
    await commit(h, ...blocks(494, 500));
    h.chain.forkAbove(496, 'b');

    // No inspect: this process never saw the fork at all. Same cursor, same
    // window, same answer — which is why one mechanism covers both.
    await expect(h.detector.bootstrap(cursorAt(500))).resolves.toEqual({
      type: 'reorg',
      firstInvalidBlock: 497,
      lastInvalidBlock: 500,
      lastValidHash: hashOf('a', 496),
    });
  });
});
