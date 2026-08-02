import { describe, expect, it } from 'vitest';

import { FakeChainClient, hashOf } from '../../test-support/fake-chain-client';
import { ForkingChain } from '../../test-support/forking-chain';
import { RecordingBlockHeaderStore } from '../../test-support/recording-block-header-store';
import {
  CHAIN_ID,
  HEAD,
  blocks,
  commit,
  cursorAt,
  harness,
  retained,
} from '../../test-support/reorg-harness';
import { ShufflingBlockHeaderStore } from '../../test-support/shuffling-block-header-store';
import { InMemoryBlockHeaderStore } from '../../test-support/in-memory-block-header-store';
import { HashChainReorgDetector } from './hash-chain-reorg-detector';
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
  // Head 1000 against a depth of 10 puts the boundary at 990, so everything
  // here sits above it. Only unsettled blocks reach `inspect` — the detector
  // waves a settled one through without looking, since it arrives as the top of
  // a range whose other blocks were never fetched.

  it('accepts a block whose parent is the header it retained', async () => {
    const h = harness();
    await commit(h, 999);

    await expect(h.detector.inspect(h.chain.headerAt(1_000))).resolves.toEqual({
      type: 'continuous',
    });
  });

  it('accepts the first block when nothing has been retained', async () => {
    const h = harness();

    await expect(h.detector.inspect(h.chain.headerAt(1_000))).resolves.toEqual({
      type: 'continuous',
    });
  });

  it('waves through a block the boundary has already settled', async () => {
    const h = harness();
    await commit(h, 999);
    // A fork under a settled block is out of scope by assumption, and the range
    // it came in with left nothing to check it against anyway.
    h.chain.forkAbove(100, 'b');

    await expect(h.detector.inspect(h.chain.headerAt(500))).resolves.toEqual({
      type: 'continuous',
    });
  });

  it('names the invalid range when the parent hash does not match', async () => {
    const h = harness();
    await commit(h, ...blocks(995, 999));
    h.chain.forkAbove(997, 'b');

    await expect(h.detector.inspect(h.chain.headerAt(1_000))).resolves.toEqual({
      type: 'reorg',
      firstInvalidBlock: 998,
      lastInvalidBlock: 999,
      lastValidHash: hashOf('a', 997),
    });
  });

  it('handles a one-block fork at the tip', async () => {
    const h = harness();
    await commit(h, ...blocks(995, 999));
    h.chain.forkAbove(998, 'b');

    await expect(h.detector.inspect(h.chain.headerAt(1_000))).resolves.toEqual({
      type: 'reorg',
      firstInvalidBlock: 999,
      lastInvalidBlock: 999,
      lastValidHash: hashOf('a', 998),
    });
  });

  it('stops rather than guessing when the window does not reach the parent block', async () => {
    const h = harness();
    await commit(h, ...blocks(995, 999));
    // A durable window and a durable cursor that drifted apart. The loop cannot
    // cause this — it never skips ahead — so it is corruption, not a fork.
    await h.store.truncate(CHAIN_ID, 997);

    await expect(h.detector.inspect(h.chain.headerAt(1_000))).resolves.toEqual({
      type: 'unrecoverable',
      reason: 'nothing retained for block 999, so block 1000 has no ancestry to check',
    });
  });

  it('catches a swap at a height it committed but never cursored', async () => {
    const h = harness();
    // The loop commits a header before saving the cursor, so a rejected save
    // leaves 1000 retained with the cursor at 999 and the loop replaying it.
    await commit(h, ...blocks(995, 1_000));
    // Only 1000 is replaced, so 999 — and therefore its parent link — still
    // checks out. Nothing but the height itself gives this away.
    h.chain.forkAbove(999, 'b');

    // Waving it through would fold the replacement on top of a block whose
    // events nothing ever told the processors to discard.
    await expect(h.detector.inspect(h.chain.headerAt(1_000))).resolves.toEqual({
      type: 'reorg',
      firstInvalidBlock: 1_000,
      lastInvalidBlock: 1_000,
      lastValidHash: hashOf('a', 999),
    });
  });

  it('tolerates re-inspecting a block it has already committed', async () => {
    const h = harness();
    await commit(h, 999, 1_000);

    // The precondition is soft: after a failed cursor save the loop replays a
    // block the detector already holds. Treating that as a violation would turn
    // a benign retry into a crash loop.
    await expect(h.detector.inspect(h.chain.headerAt(1_000))).resolves.toEqual({
      type: 'continuous',
    });
  });

  it('refuses to guess when nothing it retained is still canonical', async () => {
    const h = harness();
    await commit(h, ...blocks(995, 999));
    // Deeper than the window reaches, which is deeper than finality allows.
    h.chain.forkAbove(988, 'b');

    await expect(h.detector.inspect(h.chain.headerAt(1_000))).resolves.toEqual({
      type: 'unrecoverable',
      reason: 'no retained header between 989 and 998 is still canonical',
    });
  });

  it('does not depend on the order the store returns the window in', async () => {
    const chain = new FakeChainClient({ head: 1_000 });
    const detector = new HashChainReorgDetector(
      { chainId: CHAIN_ID, finalityDepth: 10 } as IndexingOptions,
      chain,
      new ShufflingBlockHeaderStore(),
    );
    detector.safeHead(1_000);
    await commit({ detector, chain }, ...blocks(995, 999));
    chain.forkAbove(997, 'b');

    // Nearly everything reads the window positionally — the last entry bounds
    // the invalid range, the first bounds a failed walk, and the walk descends.
    // Taken in the wrong order this under-reports, which loses writes.
    await expect(detector.inspect(chain.headerAt(1_000))).resolves.toEqual({
      type: 'reorg',
      firstInvalidBlock: 998,
      lastInvalidBlock: 999,
      lastValidHash: hashOf('a', 997),
    });
  });

  it('surfaces an RPC failure rather than mistaking it for a fork', async () => {
    const h = harness();
    await commit(h, ...blocks(995, 999));
    h.chain.forkAbove(997, 'b').failNext(1);

    await expect(h.detector.inspect(h.chain.headerAt(1_000))).rejects.toThrow('rpc unavailable');
    // The loop turns a rejection into a retry, so the window must come through
    // the failed walk untouched.
    await expect(retained(h)).resolves.toEqual(blocks(989, 999));
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
    await commit(h, ...blocks(994, 999));
    h.chain.forkAbove(996, 'b');
    const reported = await h.detector.inspect(h.chain.headerAt(1_000));

    // Died before the discard was even dispatched. The cursor was never moved,
    // so it still names 999 on the branch that lost.
    await expect(h.detector.bootstrap(cursorAt(999))).resolves.toEqual(reported);
  });

  it('reports the same fork again when the cursor landed but the rewind did not', async () => {
    const h = harness();
    await commit(h, ...blocks(994, 999));
    h.chain.forkAbove(996, 'b');
    const reported = await h.detector.inspect(h.chain.headerAt(1_000));

    // The gap the loop actually leaves: it saves the cursor before rewinding,
    // so a crash here has the cursor already naming 996 while the window still
    // holds 997..999 from the branch that lost.
    await expect(h.detector.bootstrap(cursorAt(996))).resolves.toEqual(reported);
  });

  it('stops reporting it once the rewind has also run', async () => {
    const h = harness();
    await commit(h, ...blocks(994, 999));
    h.chain.forkAbove(996, 'b');
    await h.detector.inspect(h.chain.headerAt(1_000));
    await h.detector.rewindTo(996);

    // Both writes landed, so the window and the cursor agree again and there is
    // nothing left owed.
    await expect(h.detector.bootstrap(cursorAt(996))).resolves.toEqual({ type: 'continuous' });
  });

  it('cannot tell an unapplied fork from one that happened while it was down', async () => {
    const h = harness();
    await commit(h, ...blocks(994, 999));
    h.chain.forkAbove(996, 'b');

    // No inspect: this process never saw the fork at all. Same window, same
    // answer — which is why one mechanism covers both.
    await expect(h.detector.bootstrap(cursorAt(999))).resolves.toEqual({
      type: 'reorg',
      firstInvalidBlock: 997,
      lastInvalidBlock: 999,
      lastValidHash: hashOf('a', 996),
    });
  });
});

/**
 * Reachable only once the window is durable: an in-memory `Map` cannot refuse a
 * write, so until now nothing exercised what the detector does when it is told
 * no.
 */
describe('HashChainReorgDetector — the window refuses a write', () => {
  it('surfaces a failed append rather than reporting a block it did not retain', async () => {
    const store = new RecordingBlockHeaderStore().failAppend(1);
    const h = harness(10, HEAD, store);

    await expect(commit(h, 500)).rejects.toThrow('header store unavailable');

    // The loop commits before it saves the cursor, so a throw here costs one
    // retried range and nothing else. Swallowing it would advance the cursor
    // past a block the window has no record of — a window behind its cursor,
    // which `inspect` classifies as corruption rather than a fork, and which
    // no restart can repair.
    expect(await retained(h)).toEqual([]);
  });

  it('leaves the reorg owed when the rewind cannot be written', async () => {
    const store = new RecordingBlockHeaderStore();
    const h = harness(10, HEAD, store);
    await commit(h, ...blocks(994, 999));
    h.chain.forkAbove(996, 'b');
    const reported = await h.detector.inspect(h.chain.headerAt(1_000));

    // The next one, whatever the commits above already spent.
    store.failTruncate(store.truncatedAt.length + 1);

    await expect(h.detector.rewindTo(996)).rejects.toThrow('header store unavailable');

    // The cursor was saved first, so it already names 996 while the window still
    // holds the branch that lost. That disagreement is the only record that the
    // unwind is outstanding, and it has to survive a failed write for the next
    // bootstrap to re-report the same fork.
    expect(await retained(h)).toContain(999);
    await expect(h.detector.bootstrap(cursorAt(996))).resolves.toEqual(reported);
  });
});
