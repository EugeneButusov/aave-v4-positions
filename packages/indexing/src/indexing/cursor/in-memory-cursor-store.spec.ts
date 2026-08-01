import { describe, expect, it } from 'vitest';

import type { Cursor } from './cursor-store';
import { InMemoryCursorStore } from './in-memory-cursor-store';

const HASH_A = `0x${'a'.repeat(64)}` as const;
const HASH_B = `0x${'b'.repeat(64)}` as const;

function cursor(chainId: number, lastBlock: number, lastHash = HASH_A): Cursor {
  return { chainId, lastBlock, lastHash };
}

describe('InMemoryCursorStore', () => {
  it('reports nothing for a chain it has never seen', async () => {
    await expect(new InMemoryCursorStore().load(1)).resolves.toBeNull();
  });

  it('round-trips a saved cursor', async () => {
    const store = new InMemoryCursorStore();

    await store.save(cursor(1, 500));

    await expect(store.load(1)).resolves.toEqual(cursor(1, 500));
  });

  it('overwrites rather than accumulating', async () => {
    const store = new InMemoryCursorStore();

    await store.save(cursor(1, 500));
    await store.save(cursor(1, 501, HASH_B));

    await expect(store.load(1)).resolves.toEqual(cursor(1, 501, HASH_B));
  });

  it('keeps chains apart', async () => {
    const store = new InMemoryCursorStore();

    await store.save(cursor(137, 900));

    // A cursor is scoped to its chain: reading chain 1 must not return
    // Polygon's position just because it was the only thing stored.
    await expect(store.load(1)).resolves.toBeNull();
    await expect(store.load(137)).resolves.toEqual(cursor(137, 900));
  });
});
