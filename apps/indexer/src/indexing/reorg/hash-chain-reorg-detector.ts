import { Inject, Injectable, Logger } from '@nestjs/common';

import { CHAIN_CLIENT, type BlockHeader, type ChainClient } from '../../chain/chain-client';
import { BLOCK_HEADER_STORE, type BlockHeaderStore } from './block-header-store';
import type { Cursor } from '../cursor/cursor-store';
import { INDEXING_OPTIONS, type IndexingOptions } from '../indexing.options';
import type { ReorgDetector, ReorgVerdict } from './reorg-detector';

/**
 * Detects forks by keeping the last `finalityDepth` headers it committed and
 * checking each new block's `parentHash` against them. The steady state costs
 * nothing: that hash is already in the header the loop fetched, so only a
 * mismatch spends a call, and only for as deep as the fork runs.
 *
 * Three properties hold it together, each argued where it is enforced:
 *
 * - **The window is what we processed, not what the chain says.** A header is
 *   retained only where a verified `parentHash` link ties it to a block already
 *   known to be ours ({@link fillRunBelow}); reading headers by height and
 *   trusting them looks identical and proves nothing.
 * - **It is one contiguous hash chain.** {@link commit} restarts the window
 *   when a header does not continue it, and {@link findAncestor} stops at a
 *   hole rather than assume what is inside it.
 * - **A reported fork stays owed until applied**, with nothing written down for
 *   it: an unfinished unwind leaves the cursor and the window disagreeing, and
 *   {@link bootstrap} vets the higher of the two and reports the same range
 *   until it sticks. The loop rewinds only after the cursor is durable, so the
 *   window is the side left holding the abandoned branch.
 *
 * A fork reaching at or below `safeHead` is reported, never repaired: those
 * blocks went out as settled ranges and were never inspected.
 */
@Injectable()
export class HashChainReorgDetector implements ReorgDetector {
  private readonly logger = new Logger(HashChainReorgDetector.name);

  /**
   * The head last shown to {@link safeHead}. The loop hands it over on every
   * iteration, so anything that needs the boundary mid-iteration already has
   * it and need not spend a call re-reading it.
   */
  private observedHead: number | null = null;

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

    const retained = await this.retainedWindow();
    const top = retained.at(-1);

    // The cursor is the durable commit point, but it is not always the highest
    // block the window knows about, and two partly applied steps leave it
    // behind: `commit` runs before the cursor is saved, so a rejected save
    // leaves the window one block up with those events already folded; and an
    // unwind saves the cursor before rewinding, so a crash between the two
    // leaves a whole stale run up there.
    //
    // Vetting only the cursor would answer continuous in both, and the fork
    // would then surface a beat later on the next `inspect` — except where the
    // block above has since fallen below the safe head, which is dispatched as
    // a range and never inspected at all. Vet the higher of the two instead; if
    // it is canonical, everything beneath it is.
    const ahead = top && top.number > cursor.lastBlock ? top : null;
    const highest = ahead?.number ?? cursor.lastBlock;
    const ourHash = ahead?.hash ?? cursor.lastHash;

    const canonical = await this.chain.getBlockHeader(highest);

    if (canonical.hash !== ourHash) {
      // The chain under that block is no longer the chain we folded, which is
      // precisely why the window cannot be rebuilt on this path. Reading
      // headers here would record the branch that won and erase the only
      // evidence of the one that lost, making every fork look one block deep.
      // What was durably retained is all there is to go on.
      return this.locateFork(retained, highest - 1, highest);
    }

    await this.commit(canonical);
    return { type: 'continuous' };
  }

  /**
   * Pulls in the predecessors of a header the window does not already reach, by
   * following `parentHash` down and checking every link.
   *
   * A resume and a window restart both leave a single header where a full run
   * belongs, and one header can place no fork beyond the shallowest. Each step
   * down is proven rather than assumed: the anchor is ours — the cursor says
   * so, or the loop just processed the range it tops — and its parent is ours
   * because the anchor names that exact hash.
   *
   * **Skipped below the boundary**, where a predecessor is settled and could
   * never be consulted. Not thrift: during a backfill every dispatched range
   * restarts the window, and a full pull each time would cost more calls than
   * the backfill itself. Above it this fires once, on the range that lands on
   * the boundary.
   */
  private async fillRunBelow(anchor: BlockHeader): Promise<void> {
    const safeHead = await this.currentSafeHead();

    if (anchor.number < safeHead) return;

    // A full retention window under the anchor. Clamped at zero for a chain
    // shorter than the finality depth.
    const floor = Math.max(0, anchor.number - this.options.finalityDepth);

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
    this.observedHead = observedHead;
    return Math.max(-1, observedHead - this.options.finalityDepth);
  }

  /**
   * The boundary as of the loop's current iteration.
   *
   * The loop asks {@link safeHead} before it dispatches anything, so by the
   * time a commit lands the answer is already known and no call is spent. Only
   * `bootstrap`, which runs before the loop has read a head at all, has to ask.
   * A stale or lagging value costs at worst a few headers pulled that were not
   * needed, or a window shorter than it could have been — never a wrong verdict.
   */
  private async currentSafeHead(): Promise<number> {
    return this.safeHead(this.observedHead ?? (await this.chain.getHeadBlockNumber()));
  }

  /**
   * The retained window, ascending.
   *
   * Sorted here rather than taken on trust from the adapter. Almost everything
   * below reads the window positionally — the last entry is the top the invalid
   * range is measured against, the first is the floor a failed walk reports,
   * and the walk itself must descend — so rows arriving in whatever order a
   * store happened to hand back would not fail loudly. They would under-report
   * a reorg, which is the one direction that loses writes. At `finalityDepth +
   * 1` entries the sort costs nothing worth naming.
   */
  private async retainedWindow(): Promise<BlockHeader[]> {
    const retained = await this.headers.load(this.options.chainId);
    return retained.toSorted((left, right) => left.number - right.number);
  }

  async inspect(header: BlockHeader): Promise<ReorgVerdict> {
    const retained = await this.retainedWindow();

    // Nothing retained means nothing processed: a cold start at `startBlock`,
    // with no ancestry to contradict. After bootstrap's seeding this is the
    // only way the window can be empty.
    if (retained.length === 0) return { type: 'continuous' };

    // The window can already hold this height: `commit` runs before the cursor
    // is saved, so a rejected save has the loop replay the block. Replaying the
    // *same* block is ordinary. A different one means the chain swapped it
    // under us while its events were already folded — and its parent link can
    // still check out, so the check below would wave it through.
    const existing = retained.find((entry) => entry.number === header.number);
    if (existing && existing.hash !== header.hash) {
      return this.locateFork(retained, header.number - 1, header.number);
    }

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
    const retained = await this.retainedWindow();
    const parent = retained.find((entry) => entry.number === header.number - 1);

    if (parent && parent.hash !== header.parentHash) {
      // `inspect` makes this check for every block at the tip, but a settled
      // range is dispatched without inspection — and a one-block one (the
      // range that truncates on the boundary, or a range narrowed to 1 by a
      // fussy provider) would otherwise seat a header on a parent nothing ever
      // compared it against. A break here means the chain changed at or below
      // the safe head, which the finality assumption says cannot happen and
      // which nothing else in the design would notice.
      this.logger.warn(
        `block ${header.number} does not extend retained block ${parent.number}; ` +
          'something changed at or below the safe head, so the window is being restarted',
      );
    }

    const continues = continuesTheRun(retained, header);

    if (!continues) {
      // A header that does not touch the retained run would leave a hole, and a
      // run on the far side of a hole cannot be walked back through — so it is
      // not evidence of anything and is dropped rather than kept as something
      // to trip over later. This is the ordinary case while catching up: every
      // settled range top lands clear of the last.
      await this.headers.truncate(this.options.chainId, -1);
    }

    // The floor is inclusive, so this retains `finalityDepth + 1` headers. The
    // extra one is load-bearing: the deepest recoverable fork begins at
    // `safeHead + 1`, and placing it needs its parent at exactly `safeHead`.
    const floor = Math.max(0, header.number - this.options.finalityDepth);
    await this.headers.append(this.options.chainId, header, floor);

    // Top the window back up whenever it does not reach as deep as a fork could.
    // That covers the two ways it ends up short — a restart leaves this header
    // standing alone, and a deep rewind can strip it to a couple of entries —
    // without asking which happened. In the steady state the run already
    // reaches the floor, so this is a comparison and no more.
    const oldest = continues ? retained[0] : undefined;
    const bottom = oldest ? Math.max(oldest.number, floor) : header.number;
    if (bottom > floor) await this.fillRunBelow(header);
  }

  rewindTo(lastValidBlock: number): Promise<void> {
    return this.headers.truncate(this.options.chainId, lastValidBlock);
  }

  /**
   * Turns a failed ancestry check into a verdict.
   *
   * `impliedLastInvalid` is the highest block known to have been processed on
   * the abandoned branch, and every caller works it out before walking:
   * {@link bootstrap} takes the higher of the cursor and the window, and
   * {@link inspect} is looking at the block itself. Widening it here against
   * the window's top as well would never change the answer, and an unreachable
   * guard is one a later reader trusts for a case it does not actually cover.
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
      lastInvalidBlock: impliedLastInvalid,
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
   *
   * Filling the hole from the chain is not the answer either, tempting as it
   * looks. By the time this runs the chain has already forked, so a header read
   * by height is the branch that *won*; writing it in and then comparing it
   * against itself matches trivially, names that block the common ancestor, and
   * under-reports the fork — losing exactly the writes above it that should
   * have been discarded. Whatever the window is missing has to be pulled while
   * the chain still agrees with us, which is what {@link fillRunBelow} is for.
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
 * Whether writing `header` leaves the window a single unbroken **hash** chain,
 * not merely an unbroken sequence of numbers.
 *
 * When the run already holds the block below this one, that is settled by the
 * link between them and nothing else — adjacent numbers over mismatched hashes
 * are two branches stacked on top of each other, which is worse than a hole
 * because it still looks walkable. Only where there is no such neighbour does
 * the number decide, and then only to tell an extension or a re-commit apart
 * from a jump.
 */
function continuesTheRun(retained: readonly BlockHeader[], header: BlockHeader): boolean {
  const oldest = retained[0];
  const newest = retained.at(-1);
  if (!oldest || !newest) return true;

  const parent = retained.find((entry) => entry.number === header.number - 1);
  if (parent) return parent.hash === header.parentHash;

  return header.number >= oldest.number - 1 && header.number <= newest.number + 1;
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
