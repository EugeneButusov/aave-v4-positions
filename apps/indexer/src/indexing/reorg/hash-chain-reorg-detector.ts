import { Inject, Injectable } from '@nestjs/common';

import { CHAIN_CLIENT, type BlockHeader, type ChainClient } from '../../chain/chain-client';
import { BLOCK_HEADER_STORE, type BlockHeaderStore } from './block-header-store';
import type { Cursor } from '../cursor/cursor-store';
import { INDEXING_OPTIONS, type IndexingOptions } from '../indexing.options';
import type { ReorgDetector, ReorgVerdict } from './reorg-detector';

/**
 * Detects forks by keeping the last `finalityDepth` headers it committed and
 * checking each new block's `parentHash` against them.
 *
 * **The steady state costs nothing.** The parent hash is already in the header
 * the loop fetched, so a block that extends the chain is confirmed with no RPC
 * call at all. Only a mismatch opens the wallet, and only for as many calls as
 * the fork is deep.
 *
 * **The window is what we processed, not what the chain says.** That
 * distinction is the whole design. Nothing here is ever refilled from the
 * chain: a window read back from the chain *is* the canonical chain, so every
 * retained hash would match and no fork could ever be detected. The retained
 * headers are the only evidence of the branch we followed, which is why they
 * sit behind {@link BlockHeaderStore} rather than in a field — see that port
 * for what an in-memory window costs across a restart.
 *
 * **The window is sparse below the tip.** While catching up, the loop commits
 * only the top header of each dispatched range, so the retained set has gaps.
 * The backwards walk therefore looks for the highest retained header that still
 * matches the chain rather than insisting on an unbroken parent-hash chain.
 * Where that lands on the far side of a gap, blocks that were never
 * hash-inspected get declared invalid along with the ones that were — the loop
 * re-dispatches them and the fold reproduces the same state, so the cost is
 * time, not correctness.
 *
 * The standing limitation is unchanged from the design: a fork reaching at or
 * below `safeHead` is *reported*, never repaired. Those blocks went out as
 * settled ranges and were never inspected, so the detector refuses to guess
 * which of them are wrong.
 */
@Injectable()
export class HashChainReorgDetector implements ReorgDetector {
  constructor(
    @Inject(INDEXING_OPTIONS) private readonly options: IndexingOptions,
    @Inject(CHAIN_CLIENT) private readonly chain: ChainClient,
    @Inject(BLOCK_HEADER_STORE) private readonly headers: BlockHeaderStore,
  ) {}

  /**
   * Vets the resume point against the chain, and seeds the window with it.
   *
   * `cursor.lastHash` is the one record of what we processed that survives a
   * restart on its own, so it is the only honest comparison available here. If
   * it still matches the chain, the cursor is canonical — and because a chain
   * is ancestor-closed, so is everything beneath it, which is why the ordinary
   * resume needs no walk and exactly one call.
   */
  async bootstrap(cursor: Cursor | null): Promise<ReorgVerdict> {
    if (!cursor) {
      // Nothing has been indexed, so nothing retained can be about this run.
      await this.headers.truncate(this.options.chainId, -1);
      return { type: 'continuous' };
    }

    const canonical = await this.chain.getBlockHeader(cursor.lastBlock);

    if (canonical.hash === cursor.lastHash) {
      // Seeded from a header just proven equal to what we processed — not a
      // refill from the chain, which would be worthless (see the class note).
      // Without it the window would be empty until the first commit, and a fork
      // landing before then would pass unseen and become the window's anchor.
      // It also repairs a durable window that disagreed at this height.
      await this.commit(canonical);
      return { type: 'continuous' };
    }

    const retained = await this.headers.load(this.options.chainId);
    return this.locateFork(retained, cursor.lastBlock - 1, cursor.lastBlock);
  }

  /**
   * Clamped at `-1` so a chain shorter than the finality depth — a freshly
   * started Anvil, say — yields no negative range and simply treats every block
   * as unsettled.
   */
  safeHead(observedHead: number): number {
    return Math.max(-1, observedHead - this.options.finalityDepth);
  }

  async inspect(header: BlockHeader): Promise<ReorgVerdict> {
    const retained = await this.headers.load(this.options.chainId);

    // Nothing retained means nothing processed: a cold start at `startBlock`,
    // with no ancestry to contradict. After bootstrap's seeding this is the
    // only way the window can be empty.
    if (retained.length === 0) return { type: 'continuous' };

    const parent = retained.find((entry) => entry.number === header.number - 1);
    if (parent?.hash === header.parentHash) return { type: 'continuous' };

    // Having disproved the parent height there is no point re-fetching it. When
    // the window has nothing there at all — which the loop cannot produce on
    // its own, but a durable window that drifted from the cursor can — that
    // height goes back in the search rather than being assumed good.
    const walkFrom = parent ? header.number - 2 : header.number - 1;
    return this.locateFork(retained, walkFrom, header.number - 1);
  }

  commit(header: BlockHeader): Promise<void> {
    // The floor is inclusive, so this retains `finalityDepth + 1` headers. The
    // extra one is load-bearing: the deepest recoverable fork begins at
    // `safeHead + 1`, and placing it needs its parent at exactly `safeHead`.
    return this.headers.append(
      this.options.chainId,
      header,
      header.number - this.options.finalityDepth,
    );
  }

  rewindTo(lastValidBlock: number): Promise<void> {
    return this.headers.truncate(this.options.chainId, lastValidBlock);
  }

  /**
   * Turns a failed ancestry check into a verdict.
   *
   * `impliedLastInvalid` is the highest block the caller knows was processed on
   * the abandoned branch; the window may know of a higher one, and the two
   * disagree in both directions after a partly applied step — the loop commits
   * a header before saving the cursor, and rewinds the window before saving the
   * cursor. Reporting the larger of them is what keeps the range from inverting
   * on the one side and from orphaning a block's writes on the other.
   * Over-reporting is free: `onReorg` is an idempotent discard.
   */
  private async locateFork(
    retained: readonly BlockHeader[],
    walkFrom: number,
    impliedLastInvalid: number,
  ): Promise<ReorgVerdict> {
    const ancestor = await this.findAncestor(retained, walkFrom);

    if (!ancestor) {
      return { type: 'unrecoverable', reason: this.whyUnplaceable(retained, walkFrom) };
    }

    return {
      type: 'reorg',
      firstInvalidBlock: ancestor.number + 1,
      lastInvalidBlock: Math.max(impliedLastInvalid, retained.at(-1)?.number ?? -1),
      lastValidHash: ancestor.hash,
    };
  }

  /**
   * The highest header retained at or below `from` whose hash the chain still
   * agrees with — the point the two branches share.
   *
   * Iterating the retained entries rather than a range of block numbers bounds
   * the work by the window size and steps straight over the gaps a backfill
   * leaves behind.
   */
  private async findAncestor(
    retained: readonly BlockHeader[],
    from: number,
  ): Promise<BlockHeader | null> {
    const candidates = retained.filter((entry) => entry.number <= from).toReversed();

    for (const candidate of candidates) {
      // Descending and sequential is the point: the first match is the fork
      // point, so the usual one-block reorg costs one call rather than a sweep.
      //
      // A provider failover mid-walk could answer from a node still on the old
      // branch and place the fork too high. Nothing cheap defends against that;
      // viem's `fallback` keeps steady-state traffic on the first provider.
      // oxlint-disable-next-line no-await-in-loop
      const canonical = await this.chain.getBlockHeader(candidate.number);
      if (canonical.hash === candidate.hash) return candidate;
    }

    return null;
  }

  /** Distinguishes a window that was lost from one that was merely outrun. */
  private whyUnplaceable(retained: readonly BlockHeader[], walkFrom: number): string {
    const oldest = retained[0];
    if (!oldest || oldest.number > walkFrom) {
      return `nothing retained at or below block ${walkFrom} to compare against`;
    }
    return `no retained header between ${oldest.number} and ${walkFrom} is still canonical`;
  }
}
