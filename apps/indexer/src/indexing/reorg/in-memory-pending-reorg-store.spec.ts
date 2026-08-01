import { describe, expect, it } from 'vitest';

import { hashOf } from '../../../test/fakes/fake-chain-client';
import { InMemoryPendingReorgStore } from './in-memory-pending-reorg-store';
import type { PendingReorg } from './pending-reorg-store';

function reorg(firstInvalidBlock: number, lastInvalidBlock: number): PendingReorg {
  return {
    firstInvalidBlock,
    lastInvalidBlock,
    lastValidHash: hashOf('a', firstInvalidBlock - 1),
  };
}

describe('InMemoryPendingReorgStore', () => {
  it('owes nothing on a chain it has never seen', async () => {
    await expect(new InMemoryPendingReorgStore().load(1)).resolves.toBeNull();
  });

  it('round-trips an owed reorg', async () => {
    const store = new InMemoryPendingReorgStore();

    await store.save(1, reorg(497, 500));

    await expect(store.load(1)).resolves.toEqual(reorg(497, 500));
  });

  it('owes one reorg at a time', async () => {
    const store = new InMemoryPendingReorgStore();

    await store.save(1, reorg(497, 500));
    await store.save(1, reorg(495, 496));

    // A second fork detected before the first was applied reaches at least as
    // deep, so it supersedes rather than queues behind it.
    await expect(store.load(1)).resolves.toEqual(reorg(495, 496));
  });

  it('owes nothing once cleared', async () => {
    const store = new InMemoryPendingReorgStore();
    await store.save(1, reorg(497, 500));

    await store.clear(1);

    await expect(store.load(1)).resolves.toBeNull();
  });

  it('clears a chain that owes nothing without complaint', async () => {
    const store = new InMemoryPendingReorgStore();

    await store.clear(1);

    await expect(store.load(1)).resolves.toBeNull();
  });

  it('keeps chains apart', async () => {
    const store = new InMemoryPendingReorgStore();

    await store.save(137, reorg(497, 500));

    // A reorg is scoped to its chain: unwinding Ethereum because Polygon forked
    // would be a spectacular way to corrupt both.
    await expect(store.load(1)).resolves.toBeNull();
    await expect(store.load(137)).resolves.toEqual(reorg(497, 500));
  });
});
