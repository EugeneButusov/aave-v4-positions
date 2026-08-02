import type { BlockHeader } from '../chain/chain-client';
import type { BlockHeaderStore } from '../indexing/reorg/block-header-store';

/**
 * Keeps the retention window in process memory.
 *
 * A test double, not a deployable adapter — an in-memory window under a durable
 * cursor names a resume point nothing can vet, so the only wiring that ships is
 * the Postgres one. It lives here because three other doubles build on it and
 * the detector's specs drive it directly, and it runs the same contract suite
 * the real adapter does: a fake that quietly disagrees with its port turns every
 * test using it into a proof about a fiction.
 *
 * A sorted array per chain rather than a `Map`: re-`set`ting a key keeps its
 * original insertion position, so a block re-committed after a fork would
 * silently sit in the wrong place.
 */
export class InMemoryBlockHeaderStore implements BlockHeaderStore {
  private readonly windows = new Map<number, BlockHeader[]>();

  /** A copy: a caller that mutated what it read would be writing through a read. */
  load(chainId: number): Promise<BlockHeader[]> {
    return Promise.resolve([...(this.windows.get(chainId) ?? [])]);
  }

  /** Dropping any entry at this height first is what makes the write an upsert. */
  append(chainId: number, header: BlockHeader, retainFrom: number): Promise<void> {
    const others = (this.windows.get(chainId) ?? []).filter(
      (entry) => entry.number !== header.number,
    );

    this.windows.set(
      chainId,
      [...others, header]
        .filter((entry) => entry.number >= retainFrom)
        .toSorted((left, right) => left.number - right.number),
    );
    return Promise.resolve();
  }

  truncate(chainId: number, lastValidBlock: number): Promise<void> {
    const window = this.windows.get(chainId);
    if (window) {
      this.windows.set(
        chainId,
        window.filter((entry) => entry.number <= lastValidBlock),
      );
    }
    return Promise.resolve();
  }
}
