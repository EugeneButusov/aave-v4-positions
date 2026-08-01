import type { BlockHeader, Hash } from '../../chain/chain-client';
import type { Cursor } from '../cursor/cursor-store';

/**
 * What the detector makes of a block, or of a resume point.
 *
 * - `continuous` — it extends the chain we processed; from `bootstrap`, the
 *   stored cursor is still canonical. The ordinary case.
 * - `reorg` — it does not, *and* walking back found an ancestor the chain still
 *   agrees with. Everything above that is named precisely.
 * - `unrecoverable` — it does not, and the walk exhausted the window without
 *   finding one. Rewinding blindly would delete data it cannot prove is wrong,
 *   so the loop stops and a human decides.
 */
export type ReorgVerdict =
  | { readonly type: 'continuous' }
  | {
      readonly type: 'reorg';
      /** First block on the abandoned branch. The cursor rewinds to one below it. */
      readonly firstInvalidBlock: number;
      /** Highest block we had processed on the abandoned branch. */
      readonly lastInvalidBlock: number;
      /**
       * Hash of `firstInvalidBlock - 1`, carried so the loop can write its
       * rewound cursor without a re-fetch that could itself race a second
       * reorg.
       */
      readonly lastValidHash: Hash;
    }
  | { readonly type: 'unrecoverable'; readonly reason: string };

/**
 * Owns everything the loop would otherwise know about the shape of the chain:
 * how deep a reorg can go, whether a block is settled, where a fork began. The
 * loop does no such arithmetic — it asks. Today's implementation is depth-based;
 * swapping it for one that reads `blockTag: 'finalized'` changes nothing else.
 */
export interface ReorgDetector {
  /**
   * Called once before the first iteration. Reports whether the stored cursor is
   * still canonical — the process may have been stopped across a fork, and
   * nothing else would notice.
   *
   * A detector may refill its window here, but only downwards from a block just
   * proven canonical, checking each `parentHash` link. Reading headers by height
   * is the trap: past a reorged-out resume point they describe the branch that
   * won, and recording them erases the evidence of the one that lost.
   */
  bootstrap(cursor: Cursor | null): Promise<ReorgVerdict>;

  /**
   * Highest block the detector is willing to call settled, given the head just
   * observed. Blocks at or below it are dispatched in ranges with no per-block
   * inspection; blocks above it go one at a time through {@link inspect}.
   *
   * May return a value below any real block number (on a chain shorter than the
   * finality depth, say), in which case everything is treated as unsettled.
   */
  safeHead(observedHead: number): number | Promise<number>;

  /**
   * Classify the next header.
   *
   * The loop never skips ahead, so `header.number` is at most one above the
   * highest committed — a gap would leave no way to locate a fork point. It may
   * also *equal* it, since a rejected cursor save replays the block. Treat that
   * as ordinary; asserting otherwise turns a benign retry into a crash loop.
   */
  inspect(header: BlockHeader): ReorgVerdict | Promise<ReorgVerdict>;

  /**
   * Record `header` as processed and canonical. Called only once every
   * processor has returned `ok`, and always before the cursor is saved.
   */
  commit(header: BlockHeader): void | Promise<void>;

  /**
   * Discard every retained header above `lastValidBlock`.
   *
   * **Called after the rewound cursor is saved, never before.** Retained headers
   * are the evidence of the abandoned branch; discarding them for a write that
   * has not landed leaves the detector holding less than it started with. This
   * way a crash between the two leaves the window *ahead* of the cursor, which
   * {@link bootstrap} may rely on: its highest retained header is the highest
   * block processed.
   */
  rewindTo(lastValidBlock: number): void | Promise<void>;
}

export const REORG_DETECTOR = Symbol('REORG_DETECTOR');
