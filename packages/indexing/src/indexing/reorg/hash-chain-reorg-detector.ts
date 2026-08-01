import { Inject, Injectable, Logger } from '@nestjs/common';

import { CHAIN_CLIENT, type BlockHeader, type ChainClient } from '../../chain/chain-client';
import { BLOCK_HEADER_STORE, type BlockHeaderStore } from './block-header-store';
import type { Cursor } from '../cursor/cursor-store';
import { INDEXING_OPTIONS, type IndexingOptions } from '../indexing.options';
import type { ReorgDetector, ReorgVerdict } from './reorg-detector';

/**
 * Detects forks by keeping the last `finalityDepth` headers it committed and
 * checking each new block's `parentHash` against them. The steady state costs
 * nothing — that hash is already in the header the loop fetched — so only a
 * mismatch spends a call, and only for as deep as the fork runs.
 *
 * Three properties hold it together, each argued where it is enforced:
 *
 * - **The window is what we processed, not what the chain says**, so a header
 *   is retained only behind a verified `parentHash` link ({@link fillRunBelow}).
 * - **It is one contiguous hash chain**, because {@link commit} restarts the
 *   window rather than leave a gap the walk could not cross.
 * - **A reported fork stays owed until applied**, with nothing written down for
 *   it — an unfinished unwind leaves the window holding the abandoned branch
 *   above the cursor, and {@link bootstrap} re-reports until it sticks.
 *
 * A fork at or below `safeHead` is reported, never repaired: those blocks went
 * out as settled ranges and were never inspected.
 */
@Injectable()
export class HashChainReorgDetector implements ReorgDetector {
  private readonly logger = new Logger(HashChainReorgDetector.name);

  /** The head last shown to {@link safeHead}, so nothing re-reads it mid-iteration. */
  private observedHead: number | null = null;

  constructor(
    @Inject(INDEXING_OPTIONS) private readonly options: IndexingOptions,
    @Inject(CHAIN_CLIENT) private readonly chain: ChainClient,
    @Inject(BLOCK_HEADER_STORE) private readonly headers: BlockHeaderStore,
  ) {}

  /**
   * Vets the resume point against the chain, and rebuilds the window under it.
   * One header read and no walk in the ordinary case, since a block hash commits
   * to its whole ancestry.
   *
   * The window's top is what gets vetted — it is the highest block processed,
   * never behind the cursor, because the loop commits before saving the cursor
   * and rewinds only after. Vetting the cursor would call both of those gaps
   * continuous, and nothing inspects a block that has since fallen below the
   * safe head. The cursor is the fallback for an empty window.
   *
   * A mismatch leaves the window alone: headers read by height now describe the
   * branch that won, and recording them would erase the evidence of the one that
   * lost.
   */
  async bootstrap(cursor: Cursor | null): Promise<ReorgVerdict> {
    if (!cursor) {
      await this.headers.truncate(this.options.chainId, -1);
      return { type: 'continuous' };
    }

    const retained = await this.retainedWindow();
    const top = retained.at(-1);

    const ours = top ?? { number: cursor.lastBlock, hash: cursor.lastHash };

    const canonical = await this.chain.getBlockHeader(ours.number);

    if (canonical.hash !== ours.hash) {
      return this.locateFork(retained, ours.number - 1, ours.number);
    }

    await this.commit(canonical);
    return { type: 'continuous' };
  }

  /**
   * Pulls in a header's predecessors, following `parentHash` down and checking
   * every link — a resume or a window restart leaves one header where a full run
   * belongs, and one header can place no fork beyond the shallowest.
   *
   * **Skipped below the boundary**, or a backfill would pull a full depth per
   * dispatched range and cost more calls than the backfill itself.
   */
  private async fillRunBelow(anchor: BlockHeader): Promise<void> {
    const safeHead = await this.currentSafeHead();

    if (anchor.number < safeHead) return;

    const floor = Math.max(0, anchor.number - this.options.finalityDepth);

    let child = anchor;
    while (child.number > floor) {
      // oxlint-disable-next-line no-await-in-loop
      const parent = await this.retainParentOf(child, floor);
      if (!parent) return;
      child = parent;
    }
  }

  /**
   * `null` when the chain no longer agrees this is the parent. Splicing that
   * header in would leave a window that looks like a chain and is not, so the
   * walk would later stop at a block we never processed and under-report.
   */
  private async retainParentOf(child: BlockHeader, floor: number): Promise<BlockHeader | null> {
    const parent = await this.chain.getBlockHeader(child.number - 1);

    if (parent.hash !== child.parentHash) return null;

    await this.headers.append(this.options.chainId, parent, floor);
    return parent;
  }

  /** Clamped at `-1`, so a chain shorter than the depth treats everything as unsettled. */
  safeHead(observedHead: number): number {
    this.observedHead = observedHead;
    return Math.max(-1, observedHead - this.options.finalityDepth);
  }

  /** Only `bootstrap`, running before any head is read, has to spend a call here. */
  private async currentSafeHead(): Promise<number> {
    return this.safeHead(this.observedHead ?? (await this.chain.getHeadBlockNumber()));
  }

  /** Ascending — read positionally below, so store order would under-report. */
  private async retainedWindow(): Promise<BlockHeader[]> {
    const retained = await this.headers.load(this.options.chainId);
    return retained.toSorted((left, right) => left.number - right.number);
  }

  /**
   * Classifies a header against the window.
   *
   * Three ways it is not a plain extension. The window may already hold this
   * height, since a rejected cursor save has the loop replay the block — and a
   * *different* block there was swapped under us with its events already folded,
   * which the parent check would wave through. It may hold nothing for the block
   * beneath, which the loop cannot cause, so the window and the cursor have
   * drifted and there is no ancestry left to bound a fork with. Otherwise the
   * parent link decides, and a mismatch walks from `number - 2`: the parent
   * height is already disproved, so re-reading it would buy nothing.
   *
   * An empty window means nothing was processed — a cold start, with no ancestry
   * to contradict.
   */
  async inspect(header: BlockHeader): Promise<ReorgVerdict> {
    // Settled, so nothing can reach it — and nothing could check it if it
    // could: it arrives as the top of a range whose other blocks were never
    // fetched, and the window restarts on each such top. Deciding that here
    // rather than in the loop is the point; the loop only chooses how wide to
    // dispatch, and asks about every header it commits.
    if (header.number <= (await this.currentSafeHead())) return { type: 'continuous' };

    const retained = await this.retainedWindow();

    if (retained.length === 0) return { type: 'continuous' };

    const existing = retained.find((entry) => entry.number === header.number);
    if (existing && existing.hash !== header.hash) {
      return this.locateFork(retained, header.number - 1, header.number);
    }

    const parent = retained.find((entry) => entry.number === header.number - 1);

    if (!parent) {
      return {
        type: 'unrecoverable',
        reason: `nothing retained for block ${header.number - 1}, so block ${header.number} has no ancestry to check`,
      };
    }

    if (parent.hash === header.parentHash) return { type: 'continuous' };

    return this.locateFork(retained, header.number - 2, header.number - 1);
  }

  /**
   * Records a header as processed, keeping the window one contiguous hash chain.
   *
   * A header that does not continue the run restarts it: a run on the far side
   * of a hole cannot be walked back through, so it is evidence of nothing. That
   * is the ordinary case while catching up, where each settled range top lands
   * clear of the last.
   *
   * Retains `finalityDepth + 1` headers, the floor being inclusive. The extra
   * one is load-bearing: the deepest recoverable fork starts at `safeHead + 1`,
   * and placing it needs its parent at exactly `safeHead`. The window is topped
   * back up whenever the run falls short of that floor, which covers both ways
   * it can — a restart leaving this header alone, a deep rewind stripping it.
   *
   * The warning fires where nothing else would notice: `inspect` covers the tip,
   * but a settled range is dispatched without it, so a one-block range could
   * otherwise seat a header on a parent never compared against it. A break there
   * means the chain changed at or below the safe head, which finality says
   * cannot happen.
   */
  async commit(header: BlockHeader): Promise<void> {
    const retained = await this.retainedWindow();
    const parent = retained.find((entry) => entry.number === header.number - 1);

    if (parent && parent.hash !== header.parentHash) {
      this.logger.warn(
        `block ${header.number} does not extend retained block ${parent.number}; ` +
          'something changed at or below the safe head, so the window is being restarted',
      );
    }

    const continues = continuesTheRun(retained, header);

    if (!continues) {
      await this.headers.truncate(this.options.chainId, -1);
    }

    const floor = Math.max(0, header.number - this.options.finalityDepth);
    await this.headers.append(this.options.chainId, header, floor);

    const oldest = continues ? retained[0] : undefined;
    const bottom = oldest ? Math.max(oldest.number, floor) : header.number;
    if (bottom > floor) await this.fillRunBelow(header);
  }

  rewindTo(lastValidBlock: number): Promise<void> {
    return this.headers.truncate(this.options.chainId, lastValidBlock);
  }

  /**
   * Turns a failed ancestry check into a verdict. `impliedLastInvalid` is the
   * highest block known to be on the abandoned branch; every caller has it
   * before walking.
   */
  private async locateFork(
    retained: readonly BlockHeader[],
    walkFrom: number,
    impliedLastInvalid: number,
  ): Promise<ReorgVerdict> {
    const search = await this.findAncestor(retained, walkFrom);

    if (search.kind !== 'found') {
      return { type: 'unrecoverable', reason: describeSearch(retained, walkFrom) };
    }

    return {
      type: 'reorg',
      firstInvalidBlock: search.ancestor.number + 1,
      lastInvalidBlock: impliedLastInvalid,
      lastValidHash: search.ancestor.hash,
    };
  }

  /**
   * The highest header retained at or below `from` that the chain still agrees
   * with — where the two branches meet. Descending one at a time keeps the usual
   * one-block reorg to a single call.
   *
   * Nothing here fills a gap from the chain, and {@link commit} is what makes
   * sure there is none: by the time this runs the chain has forked, so a header
   * read by height is the branch that *won* — it would match itself, be named
   * the ancestor, and under-report the fork.
   *
   * A provider failover mid-walk could answer from the old branch and place the
   * fork too high; viem's `fallback` keeps steady-state traffic on one provider.
   */
  private async findAncestor(
    retained: readonly BlockHeader[],
    from: number,
  ): Promise<AncestorSearch> {
    const candidates = retained.filter((entry) => entry.number <= from).toReversed();

    for (const candidate of candidates) {
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
  | { readonly kind: 'exhausted' };

/**
 * Whether `header` leaves the window one unbroken **hash** chain, not merely an
 * unbroken run of numbers: adjacent numbers over mismatched hashes are two
 * branches stacked on each other, worse than a hole because they still look
 * walkable. The number only decides where the block below is not retained.
 */
function continuesTheRun(retained: readonly BlockHeader[], header: BlockHeader): boolean {
  const oldest = retained[0];
  const newest = retained.at(-1);
  if (!oldest || !newest) return true;

  const parent = retained.find((entry) => entry.number === header.number - 1);
  if (parent) return parent.hash === header.parentHash;

  return header.number >= oldest.number - 1 && header.number <= newest.number + 1;
}

/** Names which way the walk ran out. */
function describeSearch(retained: readonly BlockHeader[], walkFrom: number): string {
  const oldest = retained[0];
  if (!oldest || oldest.number > walkFrom) {
    return `nothing retained at or below block ${walkFrom} to compare against`;
  }
  return `no retained header between ${oldest.number} and ${walkFrom} is still canonical`;
}
