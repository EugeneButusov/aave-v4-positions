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
 * **The window is what we processed, not what the chain says.** Every retained
 * header must be traceable to the branch we folded, never merely to the branch
 * that happens to be canonical now — which is why they sit behind
 * {@link BlockHeaderStore} rather than in a field.
 *
 * There is exactly one way to read a header from the chain and still satisfy
 * that: start from a block the cursor proves is ours and follow `parentHash`
 * down, checking each link. A hash commits to the whole ancestry beneath it, so
 * every verified step inherits the proof. {@link HashChainReorgDetector.bootstrap}
 * does that on a clean resume, and it is the difference between rebuilding the
 * window and inventing it. Reading headers by height and trusting them would
 * record whatever the chain says today; do that on a resume point that has
 * already been reorged out and it overwrites the only evidence of the branch
 * that lost, leaving every fork looking one block deep.
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
   * Vets the resume point against the chain, and rebuilds the window under it.
   *
   * `cursor.lastHash` is the one record of the branch we folded that survives a
   * restart unaided, so it is the only comparison available here — and it is a
   * complete one. A block's hash commits to its `parentHash`, which commits to
   * its parent's, all the way down, so a hash that still matches proves the
   * entire ancestry beneath it is the one we processed. That is why the
   * ordinary resume answers with a single header read and no walk: checking
   * `lastBlock - 1` could not tell us anything the check at `lastBlock` did not
   * already settle.
   */
  async bootstrap(cursor: Cursor | null): Promise<ReorgVerdict> {
    if (!cursor) {
      // Nothing has been indexed, so nothing retained can be about this run.
      await this.headers.truncate(this.options.chainId, -1);
      return { type: 'continuous' };
    }

    const canonical = await this.chain.getBlockHeader(cursor.lastBlock);

    if (canonical.hash !== cursor.lastHash) {
      // The resume point is gone, so the chain under it is no longer the chain
      // we folded — which is precisely why the window cannot be rebuilt on this
      // path. Reading headers here would record the branch that won and erase
      // the only evidence of the one that lost, making every fork look one
      // block deep. What was durably retained is all there is to go on.
      const retained = await this.headers.load(this.options.chainId);
      return this.locateFork(retained, cursor.lastBlock - 1, cursor.lastBlock);
    }

    await this.commit(canonical);
    await this.refillBelow(canonical);
    return { type: 'continuous' };
  }

  /**
   * Rebuilds the window beneath a resume point already proven to be ours, by
   * following `parentHash` down and checking every link.
   *
   * Without this the window would hold one header until the loop had committed
   * `finalityDepth` more, so a restart would leave the indexer able to place
   * only the shallowest fork for the next ~25 minutes of mainnet — and unable
   * to place anything at all on the very next block. Growing back into that
   * depth organically is the slowest possible way to reach a state the chain
   * will hand over in one pass.
   *
   * Each step is proven, not assumed. The anchor is ours because the cursor
   * says so; its parent is ours because the anchor names that exact hash as its
   * parent; and so on down. That link check is the whole difference between
   * this and reading `finalityDepth` headers by height and hoping — which would
   * silently absorb a reorg landing mid-read.
   */
  private async refillBelow(anchor: BlockHeader): Promise<void> {
    const safeHead = this.safeHead(await this.chain.getHeadBlockNumber());

    // Resuming inside the settled range, mid-backfill. The loop is about to
    // dispatch wide ranges and re-anchor the window on each one, and no fork
    // reaches this far down in the first place, so a refill here would be up to
    // `finalityDepth` calls spent on headers that the next commit discards.
    if (anchor.number <= safeHead) return;

    // Down to the deepest block a fork could still invalidate. Bounded by the
    // retention floor as well, so a lagging provider reporting an old head
    // cannot turn this into an unbounded walk.
    const floor = Math.max(0, safeHead, anchor.number - this.options.finalityDepth);

    let child = anchor;
    while (child.number > floor) {
      // Sequential by nature rather than by choice: each header names its
      // parent, and that name is what makes the next read provable.
      // oxlint-disable-next-line no-await-in-loop
      const parent = await this.retainParentOf(child, floor);
      if (!parent) return;
      child = parent;
    }
  }

  /**
   * Reads and retains `child`'s parent, if the chain still agrees that it is
   * the parent. Answers `null` when it does not.
   */
  private async retainParentOf(child: BlockHeader, floor: number): Promise<BlockHeader | null> {
    const parent = await this.chain.getBlockHeader(child.number - 1);

    // The chain moved between two reads. Everything retained so far is still
    // the branch we processed; this header is not. Splicing the two would leave
    // a window that looks like a chain and is not, and the walk would then find
    // a "shared" block that we never actually processed and under-report the
    // fork — the one error mode that loses data. Stop instead: a short window
    // reports the fork as beyond reach, which is recoverable by a human.
    if (parent.hash !== child.parentHash) return null;

    await this.headers.append(this.options.chainId, parent, floor);
    return parent;
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
