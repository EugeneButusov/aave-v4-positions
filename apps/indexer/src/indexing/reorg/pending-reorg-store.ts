import type { Hash } from '../../chain/chain-client';

/**
 * A fork that has been detected but whose unwinding is not yet known to have
 * finished. Exactly the three numbers the loop needs to dispatch it again.
 */
export interface PendingReorg {
  readonly firstInvalidBlock: number;
  readonly lastInvalidBlock: number;
  /** Hash of `firstInvalidBlock - 1`, so the rewind needs no re-fetch. */
  readonly lastValidHash: Hash;
}

/**
 * Records that a reorg is owed, from the moment it is detected until the loop
 * has demonstrably worked past it.
 *
 * Without this, a detected fork lives only in the verdict handed to the loop.
 * Crash between the detection and the cursor rewind and it is simply gone: the
 * cursor still points into the abandoned branch, and whether that is ever
 * noticed again depends on the retention window having survived — which, with
 * an in-memory window, it has not. The indexer would resume on a branch it had
 * already been told was dead.
 *
 * Re-deriving the verdict is not the same as remembering it. This is the
 * remembering, and it is deliberately independent of the window: the record
 * carries everything the rewind needs, so replaying it requires no headers, no
 * walk and no ancestry.
 *
 * **Write-before-dispatch, clear-after-progress.** The record is saved as part
 * of reporting the fork, and cleared only once the loop has committed a block
 * the fork invalidated — which it can only reach by having applied the rewind
 * and saved the cursor beneath it. Every crash in between replays the reorg,
 * and `BlockProcessor.onReorg` is an idempotent discard, so replaying costs
 * work rather than correctness.
 *
 * Both writes want to be in the same transaction as the cursor advance once a
 * database adapter exists — the same seam `CursorStore` already describes.
 */
export interface PendingReorgStore {
  /** `null` when no reorg is owed on this chain. */
  load(chainId: number): Promise<PendingReorg | null>;
  save(chainId: number, reorg: PendingReorg): Promise<void>;
  clear(chainId: number): Promise<void>;
}

export const PENDING_REORG_STORE = Symbol('PENDING_REORG_STORE');
