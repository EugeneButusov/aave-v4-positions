import { describe, expect, it } from 'vitest';

import {
  CHAIN_ID,
  blocks,
  commit,
  cursorAt,
  harness,
  retained,
} from '../../../test/fakes/reorg-harness';

describe('HashChainReorgDetector — retention', () => {
  it('never leaves a gap in the window, whatever the loop does to it', async () => {
    const h = harness(5);
    h.detector.safeHead(1_000);

    const contiguous = async (step: string) => {
      const numbers = await retained(h);
      const gaps = numbers.filter((n, i) => i > 0 && n - numbers[i - 1]! !== 1);
      expect(gaps, `${step}: ${numbers.join(',')}`).toEqual([]);
    };

    // Every shape the loop can put the window in. A gap would be unwalkable,
    // and the walk has no safe way to fill one — by then a header read by
    // height is the branch that won, so it would match itself and be named the
    // common ancestor, under-reporting the fork.
    await commit(h, 900, 901, 902);
    await contiguous('extending');
    await commit(h, 950, 951, 951, 940);
    await contiguous('jumping forward, re-committing, jumping back');
    await h.detector.rewindTo(938);
    await commit(h, 939);
    await contiguous('committing after a rewind');
    await h.detector.rewindTo(-1);
    await commit(h, 996);
    await contiguous('committing onto an emptied window');
    await h.detector.bootstrap(cursorAt(996));
    await contiguous('bootstrapping');
    h.chain.forkAbove(994, 'b');
    await h.detector.inspect(h.chain.headerAt(997));
    await commit(h, 997);
    await contiguous('committing across a fork');
  });

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
    // anything and goes. Both of these sit far below the safe head of 872, so
    // no predecessor is worth pulling: nothing could ever consult one.
    await commit(h, 149, 199);

    await expect(retained(h)).resolves.toEqual([199]);
  });

  it('pulls the predecessors of a range that lands on the boundary', async () => {
    // Safe head 872: the last range of a backfill truncates exactly onto it.
    const h = harness(128);

    await commit(h, 149, 872);

    // Inspection starts here, and a fork can reach all the way down to the
    // boundary — so the window has to arrive full rather than earn its depth
    // one block at a time.
    await expect(retained(h)).resolves.toEqual(blocks(744, 872));
  });

  it('tops the window back up after a rewind strips it', async () => {
    // Head 1000, depth 10: the tip, where forks actually happen.
    const h = harness(10);
    await commit(h, ...blocks(990, 1_000));
    // A nine-block unwind leaves two entries where eleven belong.
    await h.detector.rewindTo(991);

    await commit(h, 992);

    // Re-earning that depth one block at a time would leave the window unable
    // to size a second fork for most of the way back.
    await expect(retained(h)).resolves.toEqual(blocks(982, 992));
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

  it('restarts the window when a block does not extend the header below it', async () => {
    const h = harness();
    await commit(h, 100);
    // A change at or below the safe head, which the settled path dispatches
    // without inspecting. Adjacent numbers alone would have accepted this.
    h.chain.forkAbove(99, 'b');

    await commit(h, 101);

    // Two branches stacked on one another would still look walkable, which is
    // worse than a hole — the walk would stop at a block we never processed.
    await expect(retained(h)).resolves.toEqual([101]);
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
