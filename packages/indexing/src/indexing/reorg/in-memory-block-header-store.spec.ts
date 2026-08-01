import { describe, expect, it } from 'vitest';

import { hashOf } from '../../test-support/fake-chain-client';
import type { BlockHeader } from '../../chain/chain-client';
import { InMemoryBlockHeaderStore } from './in-memory-block-header-store';

/** No floor: keeps whatever is already retained. */
const KEEP_ALL = 0;

function header(blockNumber: number, branch = 'a'): BlockHeader {
  return {
    number: blockNumber,
    hash: hashOf(branch, blockNumber),
    parentHash: hashOf(branch, blockNumber - 1),
    timestamp: 1_700_000_000 + blockNumber * 12,
  };
}

function numbersIn(window: BlockHeader[]): number[] {
  return window.map((entry) => entry.number);
}

describe('InMemoryBlockHeaderStore', () => {
  it('reports an empty window for a chain it has never seen', async () => {
    await expect(new InMemoryBlockHeaderStore().load(1)).resolves.toEqual([]);
  });

  it('round-trips a retained header', async () => {
    const store = new InMemoryBlockHeaderStore();

    await store.append(1, header(500), KEEP_ALL);

    await expect(store.load(1)).resolves.toEqual([header(500)]);
  });

  it('returns the window ascending however it was written', async () => {
    const store = new InMemoryBlockHeaderStore();

    await store.append(1, header(502), KEEP_ALL);
    await store.append(1, header(500), KEEP_ALL);
    await store.append(1, header(501), KEEP_ALL);

    // Ascending is contractual: the detector walks the window from the top
    // down, so insertion order must not leak into the answer.
    expect(numbersIn(await store.load(1))).toEqual([500, 501, 502]);
  });

  it('overwrites a height rather than accumulating two answers for it', async () => {
    const store = new InMemoryBlockHeaderStore();

    await store.append(1, header(500, 'a'), KEEP_ALL);
    await store.append(1, header(500, 'b'), KEEP_ALL);

    // The same height is re-committed in normal operation — after a failed
    // cursor save, and after a fork with a different hash. Two entries would
    // leave the ancestor walk with two answers for one block.
    await expect(store.load(1)).resolves.toEqual([header(500, 'b')]);
  });

  it('prunes below the retention floor, keeping the floor itself', async () => {
    const store = new InMemoryBlockHeaderStore();

    await store.append(1, header(500), KEEP_ALL);
    await store.append(1, header(501), KEEP_ALL);
    await store.append(1, header(502), KEEP_ALL);
    await store.append(1, header(503), 501);

    // The floor is inclusive. It is the oldest block a fork could still be
    // traced back to, so dropping it would cost exactly the deepest recoverable
    // reorg.
    expect(numbersIn(await store.load(1))).toEqual([501, 502, 503]);
  });

  it('drops only what sits above a truncation point', async () => {
    const store = new InMemoryBlockHeaderStore();

    await store.append(1, header(500), KEEP_ALL);
    await store.append(1, header(501), KEEP_ALL);
    await store.append(1, header(502), KEEP_ALL);
    await store.truncate(1, 500);

    // 500 is the common ancestor being rewound onto, so it survives its own
    // rewind — the next block indexed is checked against it.
    expect(numbersIn(await store.load(1))).toEqual([500]);
  });

  it('clears the window when truncated below every block', async () => {
    const store = new InMemoryBlockHeaderStore();

    await store.append(1, header(500), KEEP_ALL);
    await store.truncate(1, -1);

    await expect(store.load(1)).resolves.toEqual([]);
  });

  it('truncates a chain it has never seen without complaint', async () => {
    const store = new InMemoryBlockHeaderStore();

    await store.truncate(1, -1);

    await expect(store.load(1)).resolves.toEqual([]);
  });

  it('keeps chains apart', async () => {
    const store = new InMemoryBlockHeaderStore();

    await store.append(137, header(900), KEEP_ALL);

    // A window is scoped to its chain: reading chain 1 must not return
    // Polygon's headers just because they were the only thing stored.
    await expect(store.load(1)).resolves.toEqual([]);
    expect(numbersIn(await store.load(137))).toEqual([900]);
  });

  it('hands out a copy, so a reader cannot write through a read', async () => {
    const store = new InMemoryBlockHeaderStore();
    await store.append(1, header(500), KEEP_ALL);

    (await store.load(1)).push(header(501));

    expect(numbersIn(await store.load(1))).toEqual([500]);
  });
});
