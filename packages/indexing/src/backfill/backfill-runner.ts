import { Inject, Injectable, Logger, Optional } from '@nestjs/common';

import { CHAIN_CLIENT, type ChainClient } from '../chain/chain-client';
import { BLOCK_PROCESSORS, type BlockProcessor } from '../indexing/processors/block-processor';
import { dispatchToProcessors } from '../indexing/processors/dispatch';
import {
  INDEXING_OPTIONS,
  RETRY_BASE_MS,
  RETRY_MAX_MS,
  type IndexingOptions,
} from '../indexing/indexing.options';
import { REORG_DETECTOR, type ReorgDetector } from '../indexing/reorg/reorg-detector';
import { sleepUntil } from '../sleep';

/** Consecutive retries of one range before the run gives up. */
const DEFAULT_MAX_ATTEMPTS = 5;

/** Progress is reported at every multiple of this many percent. */
const MILESTONE_PERCENT = 10;

export interface BackfillRequest {
  /** Inclusive. */
  readonly from: number;
  /** Inclusive. */
  readonly to: number;

  /** Processor names to run. Undefined or empty means every registered one. */
  readonly processors?: readonly string[];

  /** Overrides `IndexingOptions.maxRangeSize` for this run. */
  readonly rangeSize?: number;

  readonly maxAttempts?: number;

  /** Validate everything and report the plan, without dispatching. */
  readonly dryRun?: boolean;
}

export type BackfillResult =
  | { readonly kind: 'completed'; readonly ranges: number; readonly blocks: number }
  /** Interrupted. Stops at a range boundary rather than part-way through one. */
  | { readonly kind: 'aborted'; readonly resumeFrom: number }
  | { readonly kind: 'failed'; readonly reason: string; readonly resumeFrom: number };

function backoffFor(attempt: number): number {
  return Math.min(RETRY_MAX_MS, RETRY_BASE_MS * 2 ** Math.min(Math.max(0, attempt - 1), 16));
}

/**
 * Pushes one explicit block range through the registered processors, then
 * stops. The loop's counterpart for work that is asked for rather than
 * discovered: a processor whose decoding was wrong, a processor that joined
 * after the loop had already walked past the history it needs, a range being
 * checked by hand.
 *
 * Re-running a range is defined behaviour rather than a repair — dispatch is
 * at-least-once and {@link BlockProcessor} requires idempotence, so this needs
 * no coordination with the loop beyond staying off its cursor.
 *
 * **What it does not inject is the design.** No {@link CursorStore}, so it
 * cannot move the live indexer's resume point; no writable detector, so it
 * cannot truncate a header window. The detector is narrowed to `safeHead`,
 * which is the one question it asks and the only one with no side effect —
 * making the restriction the compiler's business rather than a comment's.
 *
 * Asking for `safeHead()` rather than computing `head - finalityDepth` keeps
 * that boundary defined in exactly one place, the same way the loop does.
 */
@Injectable()
export class BackfillRunner {
  private readonly logger = new Logger(BackfillRunner.name);

  constructor(
    @Inject(INDEXING_OPTIONS) private readonly options: IndexingOptions,
    @Inject(CHAIN_CLIENT) private readonly chain: ChainClient,
    @Inject(REORG_DETECTOR) private readonly boundary: Pick<ReorgDetector, 'safeHead'>,
    @Optional()
    @Inject(BLOCK_PROCESSORS)
    private readonly processors: BlockProcessor[] = [],
  ) {}

  async run(request: BackfillRequest, signal: AbortSignal): Promise<BackfillResult> {
    const blocks = request.to - request.from + 1;
    if (request.from < 0 || blocks < 1) {
      return this.refuse(request, `not a range: ${request.from}..${request.to}`);
    }

    // The same guard the loop makes on its first iteration, for the same
    // reason: pointed at the wrong chain, this writes plausible, wrong data.
    const chainId = await this.chain.getChainId();
    if (chainId !== this.options.chainId) {
      return this.refuse(
        request,
        `chain id mismatch: configured ${this.options.chainId}, provider reports ${chainId}`,
      );
    }

    const wanted = request.processors ?? [];
    const missing = wanted.filter((name) => !this.processors.some((p) => p.name === name));
    if (missing.length > 0) {
      return this.refuse(
        request,
        `unknown processor(s): ${missing.join(', ')}. Registered: ${this.registered()}`,
      );
    }

    const selected =
      wanted.length === 0
        ? this.processors
        : this.processors.filter((p) => wanted.includes(p.name));
    if (selected.length === 0) {
      return this.refuse(
        request,
        'no processors registered; there is nothing to run a range through',
      );
    }

    // Nothing here can unwind a fork, so the unsettled zone stays the loop's
    // job. Refusing rather than clamping: the range that ran should be the
    // range that was asked for.
    const safeHead = await this.boundary.safeHead(await this.chain.getHeadBlockNumber());
    if (request.to > safeHead) {
      return this.refuse(
        request,
        `block ${request.to} is above the safe head ${safeHead}; the tip is the live indexer's job`,
      );
    }

    const names = selected.map((p) => p.name).join(', ');
    let width = request.rangeSize ?? this.options.maxRangeSize;

    if (request.dryRun === true) {
      const planned = Math.ceil(blocks / width);
      this.logger.log(
        `dry run: ${blocks} block(s) ${request.from}..${request.to} in ${planned} range(s) of up to ${width}, through ${names}`,
      );
      return { kind: 'completed', ranges: planned, blocks };
    }

    this.logger.log(
      `backfilling ${blocks} block(s) ${request.from}..${request.to} through ${names}`,
    );

    const maxAttempts = request.maxAttempts ?? DEFAULT_MAX_ATTEMPTS;
    let from = request.from;
    let attempt = 0;
    let ranges = 0;
    let milestone = MILESTONE_PERCENT;

    while (from <= request.to) {
      // Checked between ranges rather than inside one: a half-dispatched range
      // has no meaning to report, and `resumeFrom` is only honest at a boundary.
      if (signal.aborted) return { kind: 'aborted', resumeFrom: from };

      const rangeFrom = from;
      const rangeTo = Math.min(from + width - 1, request.to);

      // oxlint-disable-next-line no-await-in-loop
      const outcome = await dispatchToProcessors(selected, signal, (processor, aborts) =>
        processor.onBlockRange(rangeFrom, rangeTo, aborts),
      );

      if (outcome.status === 'failed') {
        return { kind: 'failed', reason: outcome.reason, resumeFrom: rangeFrom };
      }

      if (outcome.status === 'retry') {
        attempt += 1;
        if (attempt >= maxAttempts) {
          // Bounded, unlike the loop's. Unbounded retry is right for a daemon,
          // whose wedging shows up on the readiness probe; a command that never
          // gives up just hangs a terminal with nothing watching it.
          return {
            kind: 'failed',
            reason: `gave up on blocks ${rangeFrom}..${rangeTo} after ${attempt} attempt(s): ${outcome.reason}`,
            resumeFrom: rangeFrom,
          };
        }

        if (outcome.narrowRange) {
          width = Math.max(1, Math.floor(width / 2));
          this.logger.warn(`${outcome.processor} asked to narrow; range size now ${width}`);
        }
        this.logger.warn(`attempt ${attempt} of ${maxAttempts} failed: ${outcome.reason}`);

        // oxlint-disable-next-line no-await-in-loop
        await sleepUntil(backoffFor(attempt), signal);
        continue;
      }

      attempt = 0;
      ranges += 1;
      from = rangeTo + 1;

      this.logger.debug(`blocks ${rangeFrom}..${rangeTo} done`);

      const percent = Math.floor(((rangeTo - request.from + 1) / blocks) * 100);
      if (percent >= milestone) {
        this.logger.log(`${percent}% — through block ${rangeTo}`);
        milestone = percent - (percent % MILESTONE_PERCENT) + MILESTONE_PERCENT;
      }
    }

    this.logger.log(`backfill complete: ${blocks} block(s) in ${ranges} range(s)`);
    return { kind: 'completed', ranges, blocks };
  }

  private refuse(request: BackfillRequest, reason: string): BackfillResult {
    return { kind: 'failed', reason, resumeFrom: request.from };
  }

  private registered(): string {
    return this.processors.length === 0 ? 'none' : this.processors.map((p) => p.name).join(', ');
  }
}
