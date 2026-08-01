import type { BlockHeader, Hash } from '../chain/chain-client';
import type { Cursor } from './cursor-store';

/**
 * What the detector makes of a block, or of a resume point.
 *
 * - `continuous` — the header's `parentHash` matches the hash retained for
 *   `number - 1`, so it extends the chain we already processed. From
 *   {@link ReorgDetector.bootstrap} it means the stored cursor is still on the
 *   canonical chain. This is the ordinary case.
 * - `reorg` — the parent hash does not match, *and* walking backwards found an
 *   ancestor whose hash still matches the chain. Everything above it is invalid
 *   and is named precisely.
 * - `unrecoverable` — the parent hash does not match and the walk exhausted the
 *   retention window without finding a matching ancestor. The detector cannot
 *   say which blocks are invalid, so it refuses to guess: rewinding blindly
 *   would delete data it cannot prove is wrong. The loop stops and a human
 *   decides.
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
 * Owns everything the loop would otherwise have to know about the shape of the
 * chain: how deep a reorg can go, whether a given block is settled, and where a
 * fork began. The loop performs no such arithmetic itself — it asks.
 *
 * That boundary is the point. Today's implementation is depth-based; replacing
 * it with one that reads `getBlock({ blockTag: 'finalized' })` changes this file
 * and nothing else.
 */
export interface ReorgDetector {
  /**
   * Called once, before the first iteration, with whatever the cursor store
   * held.
   *
   * Two jobs. First, fill the retention window ending at the cursor by walking
   * backwards from it, so a deep fork is resolvable immediately rather than
   * only after the window refills organically — at 128 blocks of 12s that would
   * otherwise be ~25 minutes of blindness after every restart. Second, report
   * whether the cursor is still canonical: the process may have been stopped
   * across a fork, and nothing else in the design would notice.
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
   * Precondition: `header.number === lastCommitted + 1`. The loop never skips
   * ahead — a gap in the parent-hash chain would leave no way to locate a fork
   * point.
   */
  inspect(header: BlockHeader): ReorgVerdict | Promise<ReorgVerdict>;

  /**
   * Record `header` as processed and canonical. Called only once every
   * processor has returned `ok`, and always before the cursor is saved.
   */
  commit(header: BlockHeader): void | Promise<void>;

  /** Discard every retained header above `lastValidBlock`. */
  rewindTo(lastValidBlock: number): void | Promise<void>;
}

export const REORG_DETECTOR = Symbol('REORG_DETECTOR');
