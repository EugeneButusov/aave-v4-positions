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
 * **The window is one contiguous run, and that is enforced rather than hoped
 * for.** A backwards walk over a set with holes in it is not a parent-hash
 * chain — it is a series of spot checks joined by an assumption. The assumption
 * happens to hold (a chain is ancestor-closed, so a header that still matches
 * proves its ancestors do), but leaning on it means a hole is indistinguishable
 * from a healthy window, and the walk quietly rewinds past blocks it can say
 * nothing about. So instead: {@link commit} restarts the window whenever a
 * header does not join the retained run, and the walk refuses to step over a
 * hole it nonetheless finds.
 *
 * What that means in practice. While catching up, the loop commits only the top
 * header of each dispatched range, and each of those jumps — so the window
 * collapses to that single anchor, which is exactly what the first inspected
 * block needs and all those ranges can honestly supply. From there it grows one
 * block at a time up to `finalityDepth + 1`. A hole can therefore only come
 * from a store that lost rows, and it is reported as such.
 *
 * Two limits follow, both of them the design's rather than this class's. A fork
 * reaching at or below `safeHead` is *reported*, never repaired: those blocks
 * went out as settled ranges and were never inspected, so there is nothing to
 * check them against. And for the first `finalityDepth` blocks after a backfill
 * the window is still filling, so it can bound a fork only as deep as it has
 * grown.
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

    if (!parent) {
      // The window is non-empty yet holds nothing for the block below this one.
      // The loop cannot produce that — it never skips ahead, and `commit`
      // keeps the run contiguous — so it means the window and the cursor have
      // drifted apart underneath us. There is no ancestry to check and no way
      // to bound a fork, and guessing is the one thing this class does not do.
      return {
        type: 'unrecoverable',
        reason: `nothing retained for block ${header.number - 1}, so block ${header.number} has no ancestry to check`,
      };
    }

    if (parent.hash === header.parentHash) return { type: 'continuous' };

    // Having disproved the parent height, re-fetching it would buy nothing.
    return this.locateFork(retained, header.number - 2, header.number - 1);
  }

  async commit(header: BlockHeader): Promise<void> {
    const retained = await this.headers.load(this.options.chainId);

    if (!joinsTheRun(retained, header.number)) {
      // A header that does not touch the retained run would leave a hole, and
      // a run on the far side of a hole cannot be walked back through — so it
      // is not evidence of anything and is dropped rather than kept as
      // something to trip over later. This is the ordinary case while catching
      // up: every settled range top jumps, leaving its own anchor behind.
      await this.headers.truncate(this.options.chainId, -1);
    }

    // The floor is inclusive, so this retains `finalityDepth + 1` headers. The
    // extra one is load-bearing: the deepest recoverable fork begins at
    // `safeHead + 1`, and placing it needs its parent at exactly `safeHead`.
    await this.headers.append(
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
    const search = await this.findAncestor(retained, walkFrom);

    if (search.kind !== 'found') {
      return { type: 'unrecoverable', reason: describeSearch(search, retained, walkFrom) };
    }

    return {
      type: 'reorg',
      firstInvalidBlock: search.ancestor.number + 1,
      lastInvalidBlock: Math.max(impliedLastInvalid, retained.at(-1)?.number ?? -1),
      lastValidHash: search.ancestor.hash,
    };
  }

  /**
   * The highest header retained at or below `from` whose hash the chain still
   * agrees with — the point the two branches share.
   *
   * Descends one block at a time and stops dead at a discontinuity. Stepping
   * over one would still land on a header the chain agrees with, and the fork
   * really would be above it, so the answer would be *safe* — but it would
   * cover blocks whose hashes were never recorded, quietly widening the rewind
   * to whatever the hole happened to be. A window with a hole in it is a broken
   * window; say so rather than paper over it.
   */
  private async findAncestor(
    retained: readonly BlockHeader[],
    from: number,
  ): Promise<AncestorSearch> {
    const candidates = retained.filter((entry) => entry.number <= from).toReversed();
    let expected: number | null = null;

    for (const candidate of candidates) {
      if (expected !== null && candidate.number !== expected) {
        return { kind: 'hole', missing: expected };
      }
      expected = candidate.number - 1;

      // Descending and sequential is the point: the first match is the fork
      // point, so the usual one-block reorg costs one call rather than a sweep.
      //
      // A provider failover mid-walk could answer from a node still on the old
      // branch and place the fork too high. Nothing cheap defends against that;
      // viem's `fallback` keeps steady-state traffic on the first provider.
      // oxlint-disable-next-line no-await-in-loop
      const canonical = await this.chain.getBlockHeader(candidate.number);
      if (canonical.hash === candidate.hash) return { kind: 'found', ancestor: candidate };
    }

    return { kind: 'exhausted' };
  }
}

/** How the backwards walk ended. */
type AncestorSearch =
  | { readonly kind: 'found'; readonly ancestor: BlockHeader }
  /** Ran out of retained headers before finding one the chain still agrees with. */
  | { readonly kind: 'exhausted' }
  /** The retained headers are not contiguous, so the walk could not continue. */
  | { readonly kind: 'hole'; readonly missing: number };

/**
 * Whether `blockNumber` extends, replaces or sits inside the retained run —
 * that is, whether writing it leaves the window a single unbroken sequence.
 */
function joinsTheRun(retained: readonly BlockHeader[], blockNumber: number): boolean {
  const oldest = retained[0];
  const newest = retained.at(-1);
  if (!oldest || !newest) return true;

  return blockNumber >= oldest.number - 1 && blockNumber <= newest.number + 1;
}

/** Names which of the three ways to fail actually happened. */
function describeSearch(
  search: AncestorSearch,
  retained: readonly BlockHeader[],
  walkFrom: number,
): string {
  if (search.kind === 'hole') {
    return `retained headers are not contiguous: nothing at block ${search.missing}`;
  }

  const oldest = retained[0];
  if (!oldest || oldest.number > walkFrom) {
    return `nothing retained at or below block ${walkFrom} to compare against`;
  }
  return `no retained header between ${oldest.number} and ${walkFrom} is still canonical`;
}
