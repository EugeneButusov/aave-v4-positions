import type { DecodedEvent } from '../decode/decoded-event';

/**
 * Where decoded events land.
 *
 * Two methods, because the two things a caller does with a block range are not
 * the same operation: a reorg takes rows away and leaves nothing, while a
 * dispatch takes them away and puts a fresh generation back. Folding both into
 * one call had the reorg path pass an empty array to a method named for
 * replacing, which reads as an accident rather than as the intent.
 *
 * **`revert` first, then `append`, every time a range is indexed.** Not a
 * style note. The indexing loop is at-least-once: a processor that returns `ok`
 * is re-invoked whenever a *later* one asks to retry, so the same range is
 * written more than once as a matter of course. Nothing in the storage engine
 * collapses a repeated insert on its own — measured, not assumed — so an
 * `append` without the `revert` before it leaves two live copies and doubles
 * every count and every sum taken over them. Splitting the two is what makes
 * that order visible at the call site, and also what makes it possible to get
 * wrong, so the processor's specs pin it.
 *
 * Appends only. Neither method issues a `DELETE` or mutates a row: a range is
 * emptied by writing the negation of what is in it.
 */
export interface EventStore {
  /**
   * Writes every live row in `[fromBlock, toBlock]` back with its sign flipped,
   * which is what makes the pair collapse and the range read as empty.
   *
   * Inclusive. A range holding nothing is a no-op rather than an error — the
   * loop dispatches quiet stretches like any other.
   */
  revert(chainId: number, fromBlock: number, toBlock: number): Promise<void>;

  /** Adds `events` as live rows. Writing none is a no-op. */
  append(events: readonly DecodedEvent[]): Promise<void>;
}

export const EVENT_STORE = Symbol('EVENT_STORE');
