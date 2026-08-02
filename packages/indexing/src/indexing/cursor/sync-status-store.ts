import type { Hash } from '../../chain/chain-client';

/**
 * How far the indexer has got on one chain, and how long ago it got there.
 *
 * The same row {@link Cursor} describes, read back by something that is not the
 * indexer.
 */
export interface SyncStatus {
  readonly chainId: number;
  readonly lastBlock: number;
  readonly lastHash: Hash;
  readonly updatedAt: Date;
  /**
   * Whole seconds since the indexer last advanced, **measured by the database's
   * own clock**.
   *
   * Not derivable from {@link updatedAt} by a reader: that timestamp is written
   * by the database, so subtracting it from a different process's clock reports
   * skew as staleness — and in the direction that matters, a fast reader
   * declares a healthy indexer stale.
   */
  readonly ageSeconds: number;
}

/**
 * Reads the indexer's position from outside the indexer.
 *
 * **A separate port from {@link CursorStore}, over the same row, on purpose.**
 * That one is the pipeline's single commit point — a write seam the loop owns,
 * whose whole contract is about ordering and durability. This is a read-only
 * view for a process that does not index and must not write: the API stamps
 * every payload with it, because amounts and health factors are per-block
 * quantities (§12.6) and a number with no block behind it cannot be checked.
 *
 * Also distinct from `IndexerStatus`, which is one process's live in-memory
 * state — richer, but unreachable from another service and gone when the pod
 * restarts.
 */
export interface SyncStatusStore {
  /** `null` when this chain has never been indexed. */
  get(chainId: number): Promise<SyncStatus | null>;
}

export const SYNC_STATUS_STORE = Symbol('SYNC_STATUS_STORE');
