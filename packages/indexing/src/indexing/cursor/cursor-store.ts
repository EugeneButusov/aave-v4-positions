import type { Hash } from '../../chain/chain-client';

/**
 * How far indexing has got on one chain.
 *
 * `lastHash` is non-nullable and has exactly one meaning: the hash of
 * `lastBlock` on the chain as we last saw it. The loop fetches it at the end of
 * every dispatched range — including ranges below the finality boundary, where
 * it is not needed for correctness — so that this invariant holds
 * unconditionally. That is what lets the reorg detector re-anchor from the
 * cursor alone after a restart, with no special case for "resumed mid-backfill,
 * no hash available".
 */
export interface Cursor {
  readonly chainId: number;
  readonly lastBlock: number;
  readonly lastHash: Hash;
}

/**
 * Durable position of the indexer.
 *
 * This is the **single commit point** of the whole pipeline. The loop saves it
 * last, after processors have succeeded and the detector has recorded the
 * block, so a crash anywhere earlier replays the range rather than skipping it.
 * Nothing else in the framework may durably record state the cursor does not
 * imply.
 *
 * **There is no `withTransaction` seam, and there will not be one.** It would
 * have to make a processor's writes and the cursor advance commit together, and
 * processors write to the event log's database rather than this one — so no
 * cursor store anywhere can commit atomically with them. The window remains
 * open: processor committed, cursor not yet advanced, range replayed on the
 * next start. That is the concrete reason processors must be idempotent, and
 * idempotence is the whole answer rather than a stopgap.
 *
 * What a durable adapter must not do is buffer. The loop commits the header to
 * the {@link BlockHeaderStore} before saving here, and rewinds only after, so
 * the window is never behind the cursor whichever of the two a crash lands
 * between — and that ordering only holds if each write is durable by the time
 * its promise resolves.
 */
export interface CursorStore {
  /** `null` when this chain has never been indexed. */
  load(chainId: number): Promise<Cursor | null>;
  save(cursor: Cursor): Promise<void>;
}

export const CURSOR_STORE = Symbol('CURSOR_STORE');
