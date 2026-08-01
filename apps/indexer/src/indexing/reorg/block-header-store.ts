import type { BlockHeader } from '../../chain/chain-client';

/**
 * The reorg detector's retention window: the headers it has committed recently
 * enough that a fork could still invalidate them.
 *
 * A port rather than a field on the detector because the window is the one
 * piece of reorg state that must outlive the process. On a restart the detector
 * has exactly one record of what it actually processed — `Cursor.lastHash` —
 * and one hash cannot locate a fork point. Everything below the cursor has to
 * come from here, or the fork is unplaceable and the loop stops. The in-memory
 * adapter therefore makes cross-restart recovery impossible by construction;
 * that is a property of the adapter, not of the detector.
 *
 * The detector can rebuild the window from the chain when — and only when — the
 * cursor still matches, by following `parentHash` down from that proven block
 * and checking every link. What it cannot do is rebuild it on the path that
 * needs it most: once the resume point has been reorged out, headers read by
 * height describe the branch that won, and recording those would erase the
 * evidence of the branch that lost. So a durable window is what turns a
 * cross-restart fork from unplaceable into repairable.
 *
 * Retention is the caller's policy: the floor arrives with each
 * {@link BlockHeaderStore.append}, so this layer holds no notion of finality —
 * the same split as the cursor store, which likewise stores what it is given
 * and decides nothing.
 */
export interface BlockHeaderStore {
  /**
   * Every retained header for this chain, in any order.
   *
   * Deliberately not an ordering promise. The detector reads the window
   * positionally and re-sorts what it gets, because an adapter returning rows
   * in whatever order its store handed them back would not fail loudly — it
   * would under-report a reorg, and that is the direction that loses writes. An
   * obligation whose breach is silent is not worth placing on every future
   * adapter. Returning them in block order anyway is kinder to anyone reading
   * the store directly.
   *
   * The array is the caller's to keep: an adapter must not hand out storage a
   * reader could mutate.
   *
   * What comes back should be an unbroken run of block numbers. That is the
   * detector's invariant rather than this port's to enforce — it only ever
   * writes headers that keep it — but an adapter that dropped a row in the
   * middle would break fork detection, so a hole is treated as corruption
   * rather than absorbed.
   */
  load(chainId: number): Promise<BlockHeader[]>;

  /**
   * Retain `header`, then discard everything below `retainFrom` (inclusive —
   * a header at exactly `retainFrom` survives).
   *
   * **Upserts by `(chainId, header.number)`.** The same height is written more
   * than once in normal operation: the loop re-dispatches a range whose cursor
   * save failed, and a block re-indexed after a fork arrives with a different
   * hash. Accumulating both would leave two answers for one height and make the
   * ancestor walk ambiguous.
   *
   * Pruning belongs here rather than in a separate call so a database adapter
   * can do it in one statement, with no window in which the retained set is
   * unbounded and no ordering to get wrong between the write and the prune.
   */
  append(chainId: number, header: BlockHeader, retainFrom: number): Promise<void>;

  /**
   * Discard every retained header strictly above `lastValidBlock`, which itself
   * survives — it is the common ancestor the loop is rewinding onto.
   *
   * Block numbers are non-negative, so `-1` clears the window.
   */
  truncate(chainId: number, lastValidBlock: number): Promise<void>;
}

export const BLOCK_HEADER_STORE = Symbol('BLOCK_HEADER_STORE');
