import type { BlockHeader, Hash } from '../../chain/chain-client';
import type { Cursor } from '../cursor/cursor-store';

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
   * held. Reports whether that cursor is still canonical — the process may have
   * been stopped across a fork, and nothing else in the design would notice.
   *
   * `Cursor.lastHash` is the only record of the branch we actually followed
   * that survives a restart unaided, so it is the only comparison available —
   * and a sufficient one, since a block hash commits to its whole ancestry.
   *
   * A detector that retains headers may rebuild its window here, but only
   * downwards from a block that comparison has just proven, following
   * `parentHash` and checking each link. Reading headers by height and trusting
   * them is the trap: on a resume point that has already been reorged out they
   * describe the branch that won, and recording them erases the evidence of the
   * branch that lost — leaving every fork looking one block deep. On that path
   * there is nothing to do but work with whatever was durably retained.
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
   * highest header committed — a gap in the parent-hash chain would leave no
   * way to locate a fork point. It may also *equal* it: the loop commits a
   * header before saving the cursor, so a rejected save replays the same block.
   * Treat that as ordinary. Asserting the stricter precondition would turn a
   * benign retry into a crash loop.
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
