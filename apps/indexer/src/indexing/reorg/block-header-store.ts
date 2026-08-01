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
 * Refilling the window from the chain instead is not an option, and it is worth
 * saying why, because it looks like one. A window read back from the chain *is*
 * the canonical chain, so every retained hash would match and no fork could
 * ever be detected. It would cost one RPC call per block to guarantee a wrong
 * answer.
 *
 * Retention is the caller's policy: the floor arrives with each
 * {@link BlockHeaderStore.append}, so this layer holds no notion of finality —
 * the same split as the cursor store, which likewise stores what it is given
 * and decides nothing.
 */
export interface BlockHeaderStore {
  /**
   * Every retained header for this chain, ascending by block number.
   *
   * Ascending order is part of the contract, not an accident of the
   * implementation — the detector walks the window from the top down. The array
   * is the caller's to keep: an adapter must not hand out storage a reader
   * could mutate.
   *
   * The window is **not** contiguous. While catching up, the loop commits only
   * the top header of each dispatched range, so the retained blocks are sparse
   * until it reaches the tip.
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
