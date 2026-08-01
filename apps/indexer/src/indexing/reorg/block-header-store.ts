import type { BlockHeader } from '../../chain/chain-client';

/**
 * The reorg detector's retention window: the headers it committed recently
 * enough that a fork could still invalidate them.
 *
 * A port because the window has to outlive the process. `Cursor.lastHash` is
 * the only other record of what was processed, and one hash cannot locate a
 * fork point — so an in-memory adapter makes cross-restart recovery impossible
 * by construction. The detector can refill the window from the chain only while
 * the cursor still matches; once the resume point has been reorged out, headers
 * read by height describe the branch that won.
 *
 * Retention is the caller's policy: the floor arrives with each
 * {@link BlockHeaderStore.append}, so nothing here knows about finality.
 */
export interface BlockHeaderStore {
  /**
   * Every retained header for this chain, in any order.
   *
   * No ordering promise, because breaking one would be silent: the detector
   * reads the window positionally, so rows in store order would under-report a
   * reorg rather than fail. It re-sorts instead. The array is the caller's to
   * keep.
   */
  load(chainId: number): Promise<BlockHeader[]>;

  /**
   * Retain `header`, then discard everything below `retainFrom`, which itself
   * survives.
   *
   * **Upserts by `(chainId, header.number)`.** A height is written twice in
   * normal operation — the loop re-dispatches a range whose cursor save failed,
   * and a fork re-indexes the same height with a different hash — and two
   * answers for one height would make the ancestor walk ambiguous.
   *
   * Pruning rides along so a database adapter can do both in one statement,
   * leaving no window where the retained set is unbounded.
   */
  append(chainId: number, header: BlockHeader, retainFrom: number): Promise<void>;

  /**
   * Discard every retained header above `lastValidBlock`, which survives — it is
   * the ancestor being rewound onto. `-1` clears the window.
   */
  truncate(chainId: number, lastValidBlock: number): Promise<void>;
}

export const BLOCK_HEADER_STORE = Symbol('BLOCK_HEADER_STORE');
