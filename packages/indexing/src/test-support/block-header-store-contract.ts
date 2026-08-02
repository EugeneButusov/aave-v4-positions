import { describe, expect, it } from 'vitest';

import type { BlockHeader } from '../chain/chain-client';
import type { BlockHeaderStore } from '../indexing/reorg/block-header-store';
import { hashOf } from './fake-chain-client';

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

/**
 * Sorted, because {@link BlockHeaderStore.load} promises no order. Comparing an
 * unsorted read would pass against an adapter that happens to return rows in
 * insert order and fail once its storage reorganises — which is precisely the
 * intermittent failure the port's silence about ordering is there to prevent.
 */
function numbersIn(window: BlockHeader[]): number[] {
  return window.map((entry) => entry.number).toSorted((left, right) => left - right);
}

export interface BlockHeaderStoreContract {
  /** A store retaining nothing, for any chain. Called once per test. */
  readonly fresh: () => Promise<BlockHeaderStore>;
}

/**
 * The {@link BlockHeaderStore} port as an executable specification.
 *
 * Every adapter runs this, because the detector reads the window positionally
 * and an adapter that got any of it subtly wrong would under-report a reorg
 * rather than fail — the retained set is the only evidence of what was
 * processed, so there is nothing left to cross-check it against. Two
 * hand-written suites would drift, and the one that drifted would be the
 * durable one nobody runs locally.
 */
export function describeBlockHeaderStoreContract(
  name: string,
  contract: BlockHeaderStoreContract,
): void {
  describe(`${name} — BlockHeaderStore contract`, () => {
    it('reports an empty window for a chain it has never seen', async () => {
      const store = await contract.fresh();

      await expect(store.load(1)).resolves.toEqual([]);
    });

    it('round-trips a retained header', async () => {
      const store = await contract.fresh();

      await store.append(1, header(500), KEEP_ALL);

      await expect(store.load(1)).resolves.toEqual([header(500)]);
    });

    it('overwrites a height rather than accumulating two answers for it', async () => {
      const store = await contract.fresh();

      await store.append(1, header(500, 'a'), KEEP_ALL);
      await store.append(1, header(500, 'b'), KEEP_ALL);

      // The same height is re-committed in normal operation — after a failed
      // cursor save, and after a fork with a different hash. Two entries would
      // leave the ancestor walk with two answers for one block.
      await expect(store.load(1)).resolves.toEqual([header(500, 'b')]);
    });

    it('prunes below the retention floor, keeping the floor itself', async () => {
      const store = await contract.fresh();

      await store.append(1, header(500), KEEP_ALL);
      await store.append(1, header(501), KEEP_ALL);
      await store.append(1, header(502), KEEP_ALL);
      await store.append(1, header(503), 501);

      // The floor is inclusive. It is the oldest block a fork could still be
      // traced back to, so dropping it would cost exactly the deepest
      // recoverable reorg.
      expect(numbersIn(await store.load(1))).toEqual([501, 502, 503]);
    });

    it('prunes only the chain it was given a floor for', async () => {
      const store = await contract.fresh();

      await store.append(137, header(900), KEEP_ALL);
      await store.append(137, header(901), KEEP_ALL);
      await store.append(1, header(1000), 999);

      // A database adapter prunes with a DELETE, and a DELETE that forgets its
      // chain predicate wipes every other chain's window. Nothing else in this
      // suite can see that: single-chain tests pass either way, and the damage
      // only appears on a deployment indexing more than one chain.
      expect(numbersIn(await store.load(137))).toEqual([900, 901]);
    });

    it('drops only what sits above a truncation point', async () => {
      const store = await contract.fresh();

      await store.append(1, header(500), KEEP_ALL);
      await store.append(1, header(501), KEEP_ALL);
      await store.append(1, header(502), KEEP_ALL);
      await store.truncate(1, 500);

      // 500 is the common ancestor being rewound onto, so it survives its own
      // rewind — the next block indexed is checked against it.
      expect(numbersIn(await store.load(1))).toEqual([500]);
    });

    it('truncates only the chain it was asked to', async () => {
      const store = await contract.fresh();

      await store.append(137, header(900), KEEP_ALL);
      await store.append(1, header(500), KEEP_ALL);
      await store.truncate(1, -1);

      expect(numbersIn(await store.load(137))).toEqual([900]);
    });

    it('clears the window when truncated below every block', async () => {
      const store = await contract.fresh();

      await store.append(1, header(500), KEEP_ALL);
      await store.truncate(1, -1);

      await expect(store.load(1)).resolves.toEqual([]);
    });

    it('truncates a chain it has never seen without complaint', async () => {
      const store = await contract.fresh();

      await store.truncate(1, -1);

      await expect(store.load(1)).resolves.toEqual([]);
    });

    it('keeps chains apart on read', async () => {
      const store = await contract.fresh();

      await store.append(137, header(900), KEEP_ALL);

      // A window is scoped to its chain: reading chain 1 must not return
      // Polygon's headers just because they were the only thing stored.
      await expect(store.load(1)).resolves.toEqual([]);
      expect(numbersIn(await store.load(137))).toEqual([900]);
    });

    it('promises nothing about order, so every height written comes back', async () => {
      const store = await contract.fresh();

      await store.append(1, header(502), KEEP_ALL);
      await store.append(1, header(500), KEEP_ALL);
      await store.append(1, header(501), KEEP_ALL);

      // Asserted as a set, not a sequence. The detector sorts what it reads, and
      // an adapter that returned rows without an ORDER BY is explicitly allowed
      // — see ShufflingBlockHeaderStore, which exists to prove the detector does
      // not lean on it.
      expect(numbersIn(await store.load(1))).toEqual([500, 501, 502]);
    });

    it('hands out an array the caller may keep', async () => {
      const store = await contract.fresh();
      await store.append(1, header(500), KEEP_ALL);

      (await store.load(1)).push(header(501));

      expect(numbersIn(await store.load(1))).toEqual([500]);
    });
  });
}
