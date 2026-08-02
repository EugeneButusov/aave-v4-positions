import { describe, expect, it } from 'vitest';

import type { Cursor, CursorStore } from '../indexing/cursor/cursor-store';
import { hashOf } from './fake-chain-client';

function cursor(chainId: number, lastBlock: number, branch = 'a'): Cursor {
  return { chainId, lastBlock, lastHash: hashOf(branch, lastBlock) };
}

export interface CursorStoreContract {
  /** A store holding no cursor, for any chain. Called once per test. */
  readonly fresh: () => Promise<CursorStore>;
}

/**
 * The {@link CursorStore} port as an executable specification.
 *
 * Short, because the port is two methods — but it is the pipeline's single
 * commit point, so an adapter that answers any of these differently changes
 * where the indexer resumes rather than failing anything.
 */
export function describeCursorStoreContract(name: string, contract: CursorStoreContract): void {
  describe(`${name} — CursorStore contract`, () => {
    it('reports nothing for a chain it has never seen', async () => {
      const store = await contract.fresh();

      await expect(store.load(1)).resolves.toBeNull();
    });

    it('round-trips a saved cursor', async () => {
      const store = await contract.fresh();

      await store.save(cursor(1, 500));

      await expect(store.load(1)).resolves.toEqual(cursor(1, 500));
    });

    it('overwrites rather than accumulating', async () => {
      const store = await contract.fresh();

      await store.save(cursor(1, 500));
      await store.save(cursor(1, 501));

      await expect(store.load(1)).resolves.toEqual(cursor(1, 501));
    });

    it('accepts a cursor that moves backwards', async () => {
      const store = await contract.fresh();

      await store.save(cursor(1, 500, 'a'));
      await store.save(cursor(1, 499, 'b'));

      // Every reorg rewind is this: `applyReorg` saves the last valid block,
      // which is below where the cursor already stood. An adapter that added the
      // monotonic guard a reviewer reaches for — `WHERE last_block < excluded` —
      // would accept the write, report success, and silently discard the unwind,
      // leaving the loop continuing from a branch that lost.
      await expect(store.load(1)).resolves.toEqual(cursor(1, 499, 'b'));
    });

    it('keeps chains apart', async () => {
      const store = await contract.fresh();

      await store.save(cursor(137, 900));

      // A cursor is scoped to its chain: reading chain 1 must not return
      // Polygon's position just because it was the only thing stored.
      await expect(store.load(1)).resolves.toBeNull();
      await expect(store.load(137)).resolves.toEqual(cursor(137, 900));
    });
  });
}
