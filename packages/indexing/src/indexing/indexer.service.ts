import {
  Inject,
  Injectable,
  Logger,
  Optional,
  type OnApplicationBootstrap,
  type OnApplicationShutdown,
} from '@nestjs/common';

import { CHAIN_CLIENT, type BlockHeader, type ChainClient, type Hash } from '../chain/chain-client';
import {
  BLOCK_PROCESSORS,
  type BlockProcessor,
  type ProcessorOutcome,
} from './processors/block-processor';
import { CURSOR_STORE, type Cursor, type CursorStore } from './cursor/cursor-store';
import { IndexerStatus } from './observability/indexer-status';
import {
  INDEXING_OPTIONS,
  RETRY_BASE_MS,
  RETRY_MAX_MS,
  SHUTDOWN_DRAIN_MS,
  type IndexingOptions,
} from './indexing.options';
import { REORG_DETECTOR, type ReorgDetector } from './reorg/reorg-detector';

/**
 * What one iteration accomplished. Drives both the backoff and, through
 * `lastProgressAt`, the stall alarm on readiness.
 */
export type IterationResult =
  /** Work was dispatched and the cursor moved. Includes a reorg rewind. */
  | { readonly kind: 'progressed'; readonly cursorAt: number }
  /** Caught up to the head; nothing to do until a new block arrives. */
  | { readonly kind: 'idle' }
  | { readonly kind: 'retry'; readonly reason: string }
  | { readonly kind: 'failed'; readonly reason: string };

function describe(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Walks the chain and hands ranges to the registered processors.
 *
 * The loop knows about block numbers and nothing else. It holds no notion of
 * finality, of what a fork looks like, or of what any processor does with a
 * range — those live behind {@link ReorgDetector} and {@link BlockProcessor}
 * respectively, and every chain-shape question is answered by asking rather
 * than by arithmetic here.
 *
 * Each iteration takes one of two paths, chosen fresh from the head every time
 * rather than latched as a mode:
 *
 * - **At or below the detector's safe head**, blocks are settled, so they are
 *   dispatched in ranges of up to `maxRangeSize` with no per-block inspection.
 *   This is what makes a backfill of ~932k blocks (analysis §8) take minutes.
 * - **Above it**, one block at a time, inspected before it is dispatched.
 *
 * A range never straddles the boundary: half of it would be covered by a
 * finality assumption and half not, and the single hash committed for its top
 * block would sit inside the reorg window with no ancestry beneath it.
 */
@Injectable()
export class IndexerService implements OnApplicationBootstrap, OnApplicationShutdown {
  private readonly logger = new Logger(IndexerService.name);
  private readonly abort = new AbortController();

  private cursor: Cursor | null = null;
  private chainIdVerified = false;
  private bootstrapped = false;
  private effectiveMaxRange: number;
  private started = false;
  private runner: Promise<void> | undefined;

  constructor(
    @Inject(INDEXING_OPTIONS) private readonly options: IndexingOptions,
    @Inject(CHAIN_CLIENT) private readonly chain: ChainClient,
    @Inject(REORG_DETECTOR) private readonly detector: ReorgDetector,
    @Inject(CURSOR_STORE) private readonly cursorStore: CursorStore,
    private readonly status: IndexerStatus,
    @Optional()
    @Inject(BLOCK_PROCESSORS)
    private readonly processors: BlockProcessor[] = [],
  ) {
    this.effectiveMaxRange = options.maxRangeSize;
  }

  onApplicationBootstrap(): void {
    if (!this.options.autoStart) {
      this.logger.log('autostart disabled; not indexing');
      return;
    }
    this.start();
  }

  start(): void {
    if (this.started) return;
    this.started = true;
    this.runner = this.run();
  }

  async onApplicationShutdown(): Promise<void> {
    this.abort.abort();
    await this.drain();
  }

  /**
   * One pass, including the state transition it implies. Public because it is
   * the unit the tests drive: nearly every behaviour below can be asserted by
   * calling this directly, with no timers and no background task to wind down.
   *
   * `failed` is terminal here rather than only in {@link run}, so the state is
   * correct however the service is driven and a caller cannot resurrect a
   * stopped indexer by calling again.
   */
  async runOnce(): Promise<IterationResult> {
    if (this.status.isFailed) {
      return { kind: 'failed', reason: this.status.failureReason ?? 'indexing stopped' };
    }
    return this.record(await this.iterate());
  }

  /** Translates what happened into a state transition. */
  private record(result: IterationResult): IterationResult {
    switch (result.kind) {
      case 'failed':
        this.status.failed(result.reason);
        break;
      case 'retry':
        this.status.retried(result.reason);
        break;
      case 'progressed':
        this.status.progressed(result.cursorAt);
        break;
      default:
        this.status.idled();
    }
    return result;
  }

  private async iterate(): Promise<IterationResult> {
    try {
      const identity = await this.verifyChainId();
      if (identity) return identity;

      const resumed = await this.bootstrap();
      if (resumed) return resumed;

      // Clamped to the high-water mark inside the status — a provider failover
      // can otherwise make the head appear to move backwards.
      const head = this.status.observeHead(await this.chain.getHeadBlockNumber());

      const next = this.cursor ? this.cursor.lastBlock + 1 : this.options.startBlock;
      if (next > head) return { kind: 'idle' };

      // The boundary decides how wide to dispatch and nothing else. At or below
      // it the blocks are settled, so they go out in ranges — which is what
      // makes a ~932k-block backfill take minutes; above it, one at a time. A
      // range never straddles it.
      const safeHead = await this.detector.safeHead(head);
      const to = next <= safeHead ? Math.min(next + this.effectiveMaxRange - 1, safeHead) : next;

      // Awaited inside the try rather than returned from it: a returned promise
      // settles after the try block has exited, so a rejection would escape
      // this catch and take down the loop.
      return await this.indexRange(next, to);
    } catch (error) {
      // Reached only by a fault outside a processor — an RPC failure, a cursor
      // store throwing. Retries are unbounded, so a genuine code defect keeps
      // failing here and surfaces through the stall alarm rather than being
      // mistaken for a terminal condition.
      return { kind: 'retry', reason: `unhandled: ${describe(error)}` };
    }
  }

  /**
   * Checked once, lazily, rather than in the DI factory: a boot that reaches
   * out to an RPC node turns a provider outage into a crash loop, where what we
   * want is a pod that starts and reports not-ready.
   */
  private async verifyChainId(): Promise<IterationResult | null> {
    if (this.chainIdVerified) return null;

    const actual = await this.chain.getChainId();
    if (actual !== this.options.chainId) {
      return {
        kind: 'failed',
        reason: `chain id mismatch: configured ${this.options.chainId}, provider reports ${actual}`,
      };
    }

    this.chainIdVerified = true;
    return null;
  }

  /**
   * Loads the cursor and lets the detector fill its window and vet the resume
   * point. `bootstrapped` is set only once the whole step has succeeded, so a
   * transient failure re-runs it rather than leaving the detector half-primed.
   */
  private async bootstrap(): Promise<IterationResult | null> {
    if (this.bootstrapped) return null;

    const stored = await this.cursorStore.load(this.options.chainId);
    const verdict = await this.detector.bootstrap(stored);

    if (verdict.type === 'unrecoverable') {
      return { kind: 'failed', reason: `cannot resume: ${verdict.reason}` };
    }

    if (verdict.type === 'reorg') {
      // The chain forked while the process was down. Nothing else in the design
      // would notice: the detector's window starts empty, so without this the
      // loop would carry on from the cursor onto a branch that lost.
      this.logger.warn(
        `reorg across restart: blocks ${verdict.firstInvalidBlock}..${verdict.lastInvalidBlock} are stale`,
      );
      const rewound = await this.applyReorg(
        verdict.firstInvalidBlock,
        verdict.lastInvalidBlock,
        verdict.lastValidHash,
      );
      if (rewound.kind !== 'progressed') return rewound;

      this.bootstrapped = true;
      return rewound;
    }

    this.cursor = stored;
    this.bootstrapped = true;
    if (stored) {
      this.logger.log(`resuming from block ${stored.lastBlock}`);
    } else {
      this.logger.log(`no stored cursor; starting at block ${this.options.startBlock}`);
    }
    return null;
  }

  /**
   * Indexes `[from, to]`, inclusive, having asked the detector about it first.
   *
   * One path for settled and unsettled alike. The width is the only thing that
   * differs, and the width is all the loop decides: a wide range means one
   * header read for many blocks, so the top is the only header there is to
   * anchor on or ask about. Whether that header needs checking is the
   * detector's call, not this one's — it knows where the boundary is, and a
   * settled top it can wave through without looking.
   *
   * Reading the top header even for a settled range costs one call per range —
   * ~940 across the whole backfill — and is what lets `Cursor.lastHash` mean the
   * same thing always, so the detector can re-anchor from the cursor alone.
   */
  private async indexRange(from: number, to: number): Promise<IterationResult> {
    const header = await this.chain.getBlockHeader(to);

    const forked = await this.verifyAncestry(header);
    if (forked) return forked;

    const failure = await this.dispatch((p, signal) => p.onBlockRange(from, to, signal));
    if (failure) return failure;

    await this.detector.commit(header);
    await this.saveCursor(header.number, header.hash);

    return { kind: 'progressed', cursorAt: to };
  }

  /**
   * Asks the detector whether this header extends the chain we folded, and
   * answers `null` when it does — the same shape as {@link verifyChainId} and
   * {@link bootstrap}, where a value means the iteration is already decided.
   *
   * It does not only report. A fork is unwound here, which dispatches the
   * discard and moves the cursor, so the `progressed` that comes back is the
   * rewind rather than the range the caller was about to index.
   */
  private async verifyAncestry(header: BlockHeader): Promise<IterationResult | null> {
    const verdict = await this.detector.inspect(header);

    if (verdict.type === 'unrecoverable') {
      return { kind: 'failed', reason: `reorg beyond the detector's reach: ${verdict.reason}` };
    }

    if (verdict.type === 'reorg') {
      this.logger.warn(
        `reorg at block ${header.number}: blocks ${verdict.firstInvalidBlock}..${verdict.lastInvalidBlock} are stale`,
      );
      return this.applyReorg(
        verdict.firstInvalidBlock,
        verdict.lastInvalidBlock,
        verdict.lastValidHash,
      );
    }

    return null;
  }

  /**
   * Unwinds a fork. Nothing is mutated until every processor has accepted the
   * reorg, so a failure there leaves the step entirely unapplied and retried.
   *
   * **The rewind is last, after the cursor is durable**, because it discards the
   * headers that are the evidence of the abandoned branch. Rewinding first and
   * then failing the save would leave the detector holding less than it started
   * with, having committed to nothing; this way it holds more — stale headers
   * above an already-correct cursor — which `bootstrap` resolves by re-reporting
   * the range for an idempotent second discard.
   */
  private async applyReorg(
    firstInvalidBlock: number,
    lastInvalidBlock: number,
    lastValidHash: Hash,
  ): Promise<IterationResult> {
    const failure = await this.dispatch((p, signal) =>
      p.onReorg(firstInvalidBlock, lastInvalidBlock, signal),
    );
    if (failure) return failure;

    const lastValidBlock = firstInvalidBlock - 1;
    await this.saveCursor(lastValidBlock, lastValidHash);
    await this.detector.rewindTo(lastValidBlock);

    return { kind: 'progressed', cursorAt: lastValidBlock };
  }

  /**
   * Runs the processors in registration order, stopping at the first non-`ok`.
   * Returns `null` when all succeeded.
   *
   * Fail-fast means an earlier processor is re-invoked when a later one asks to
   * retry, which is why the interface requires idempotence.
   *
   * TODO: parallelize once the real processors land — the Spoke, Hub and price
   * streams have no ordering dependency during backfill (analysis §8). Needs
   * `allSettled`, not `all`: outcomes are returned rather than thrown, so
   * `Promise.all` cannot fail fast here, and it does not cancel its siblings
   * either. Also needs a concurrency cap for the measured provider rate limits,
   * `narrowRange` halving once per dispatch rather than once per processor that
   * asks, and registration order dropped from the {@link BlockProcessor}
   * contract.
   */
  private async dispatch(
    invoke: (
      processor: BlockProcessor,
      signal: AbortSignal,
    ) => ProcessorOutcome | Promise<ProcessorOutcome>,
  ): Promise<IterationResult | null> {
    for (const processor of this.processors) {
      let outcome: ProcessorOutcome;
      try {
        // oxlint-disable-next-line no-await-in-loop
        outcome = await invoke(processor, this.abort.signal);
      } catch (error) {
        // A thrown error is a retry, not a failure. It is indistinguishable
        // from one at this level, and treating it as terminal would let a
        // transient bug stop indexing permanently.
        return { kind: 'retry', reason: `${processor.name} threw: ${describe(error)}` };
      }

      if (outcome.status === 'failed') {
        return { kind: 'failed', reason: `${processor.name}: ${outcome.reason}` };
      }

      if (outcome.status === 'retry') {
        if (outcome.narrowRange === true) this.narrowRange(processor.name);
        return { kind: 'retry', reason: `${processor.name}: ${outcome.reason}` };
      }
    }

    return null;
  }

  private narrowRange(processorName: string): void {
    this.effectiveMaxRange = Math.max(1, Math.floor(this.effectiveMaxRange / 2));
    this.logger.warn(`${processorName} asked to narrow; range size now ${this.effectiveMaxRange}`);
  }

  /**
   * The single durable commit point, written last. A crash before this replays
   * the range; a crash after it does not. `this.cursor` is only updated once
   * the store has accepted the write, so a failing store leaves the loop
   * pointing at the last position it actually persisted.
   */
  private async saveCursor(lastBlock: number, lastHash: Hash): Promise<void> {
    const cursor: Cursor = { chainId: this.options.chainId, lastBlock, lastHash };
    await this.cursorStore.save(cursor);
    this.cursor = cursor;
  }

  private async run(): Promise<void> {
    this.logger.log(
      `indexing chain ${this.options.chainId} via ${this.options.rpcUrls.length} provider(s)`,
    );

    while (!this.abort.signal.aborted) {
      // A scheduler: each iteration depends on the last having finished, and
      // overlapping two would race the cursor.
      // oxlint-disable-next-line no-await-in-loop
      const result = await this.runOnce();

      if (result.kind === 'failed') {
        // Terminal by design. Nothing here resets it — a restart does, which is
        // the same posture the rest of the service takes towards bad config.
        this.logger.fatal(`indexing stopped: ${result.reason}`);
        return;
      }

      if (result.kind === 'retry') {
        this.logger.warn(
          `iteration failed (attempt ${this.status.consecutiveFailures}): ${result.reason}`,
        );
      }

      // oxlint-disable-next-line no-await-in-loop
      await this.sleep(this.delayFor(result));
    }

    this.status.stopped();
  }

  private delayFor(result: IterationResult): number {
    if (result.kind === 'progressed') return 0;
    if (result.kind === 'idle') return this.options.pollIntervalMs;

    const exponent = Math.min(this.status.consecutiveFailures - 1, 16);
    return Math.min(RETRY_MAX_MS, RETRY_BASE_MS * 2 ** Math.max(0, exponent));
  }

  /** Wakes on the delay or on shutdown, so a quiet loop does not sit out its poll interval. */
  private sleep(ms: number): Promise<void> {
    const { signal } = this.abort;
    if (ms <= 0 || signal.aborted) return Promise.resolve();

    // Composing the two signals rather than racing a timer against a listener:
    // there is one wake path, so no cleanup to get wrong and no way to settle
    // twice. The timeout signal does not hold the event loop open.
    const wake = AbortSignal.any([signal, AbortSignal.timeout(ms)]);
    return new Promise<void>((resolve) => {
      wake.addEventListener('abort', () => resolve(), { once: true });
    });
  }

  private async drain(): Promise<void> {
    if (!this.runner) return;

    let timer: NodeJS.Timeout | undefined;
    const expiry = new Promise<void>((resolve) => {
      timer = setTimeout(resolve, SHUTDOWN_DRAIN_MS);
    });

    try {
      await Promise.race([this.runner, expiry]);
    } finally {
      clearTimeout(timer);
    }
  }
}
