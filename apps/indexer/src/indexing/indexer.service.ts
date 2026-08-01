import {
  Inject,
  Injectable,
  Logger,
  Optional,
  type OnApplicationBootstrap,
  type OnApplicationShutdown,
} from '@nestjs/common';

import { CHAIN_CLIENT, type ChainClient, type Hash } from '../chain/chain-client';
import { BLOCK_PROCESSORS, type BlockProcessor, type ProcessorOutcome } from './block-processor';
import { CURSOR_STORE, type Cursor, type CursorStore } from './cursor-store';
import {
  INDEXING_OPTIONS,
  RETRY_BASE_MS,
  RETRY_MAX_MS,
  SHUTDOWN_DRAIN_MS,
  type IndexingOptions,
} from './indexing.options';
import { REORG_DETECTOR, type ReorgDetector } from './reorg-detector';

export type IndexerState = 'starting' | 'running' | 'retrying' | 'failed' | 'stopped';

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

export interface IndexerSnapshot {
  readonly state: IndexerState;
  readonly reason: string | null;
  readonly lastBlock: number | null;
  readonly head: number | null;
  readonly lastProgressAt: number;
}

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

  private state: IndexerState = 'starting';
  private failureReason: string | null = null;
  private cursor: Cursor | null = null;
  private head = -1;
  private chainIdVerified = false;
  private bootstrapped = false;
  /**
   * The range width actually in use, which is not `options.maxRangeSize` — that
   * is the configured ceiling, and this drops below it for good the first time a
   * processor answers `narrowRange`. It cannot live on the options object:
   * those are readonly, and the same instance is shared with the chain adapter
   * under `CHAIN_CLIENT_OPTIONS`, so narrowing there would be a module-wide edit
   * to record one loop's local backoff.
   */
  private effectiveMaxRange: number;
  private attempts = 0;
  private lastProgressAt = Date.now();
  private started = false;
  private runner: Promise<void> | undefined;

  constructor(
    @Inject(INDEXING_OPTIONS) private readonly options: IndexingOptions,
    @Inject(CHAIN_CLIENT) private readonly chain: ChainClient,
    @Inject(REORG_DETECTOR) private readonly detector: ReorgDetector,
    @Inject(CURSOR_STORE) private readonly cursorStore: CursorStore,
    @Optional()
    @Inject(BLOCK_PROCESSORS)
    private readonly processors: BlockProcessor[] = [],
  ) {
    this.effectiveMaxRange = options.maxRangeSize;
  }

  /** Everything the health indicator needs, and nothing it can mutate. */
  get snapshot(): IndexerSnapshot {
    return {
      state: this.state,
      reason: this.failureReason,
      lastBlock: this.cursor?.lastBlock ?? null,
      head: this.head < 0 ? null : this.head,
      lastProgressAt: this.lastProgressAt,
    };
  }

  onApplicationBootstrap(): void {
    if (!this.options.autoStart) {
      this.logger.log('autostart disabled; not indexing');
      return;
    }
    this.start();
  }

  /**
   * The loop is deliberately not awaited — boot would never finish. Assigning
   * the promise rather than discarding it keeps it available to shutdown, and
   * satisfies `no-floating-promises` without a `void` that would also discard
   * the handle.
   */
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
    if (this.state === 'failed') {
      return { kind: 'failed', reason: this.failureReason ?? 'indexing stopped' };
    }
    return this.record(await this.iterate());
  }

  private record(result: IterationResult): IterationResult {
    if (result.kind === 'failed') {
      this.state = 'failed';
      this.failureReason = result.reason;
    } else if (result.kind === 'retry') {
      this.state = 'retrying';
      this.attempts += 1;
    } else {
      this.state = 'running';
      this.attempts = 0;
      // Idle counts as progress: being caught up is the healthy steady state,
      // and the stall alarm must not fire on a quiet chain.
      this.lastProgressAt = Date.now();
    }
    return result;
  }

  private async iterate(): Promise<IterationResult> {
    try {
      const identity = await this.verifyChainId();
      if (identity) return identity;

      const resumed = await this.bootstrap();
      if (resumed) return resumed;

      const observed = await this.chain.getHeadBlockNumber();
      if (observed < this.head) {
        // Providers are not synchronised, and viem's fallback does not
        // reconcile them: failing over to a lagging node makes the head appear
        // to move backwards, which is otherwise indistinguishable from a reorg.
        this.logger.warn(`head regressed ${this.head} -> ${observed}; clamping to the high mark`);
      }
      this.head = Math.max(this.head, observed);

      const next = this.cursor ? this.cursor.lastBlock + 1 : this.options.startBlock;
      if (next > this.head) return { kind: 'idle' };

      const safeHead = await this.detector.safeHead(this.head);

      // Awaited inside the try rather than returned from it: a returned promise
      // settles after the try block has exited, so a rejection from either
      // branch would escape this catch and take down the loop.
      const result =
        next <= safeHead
          ? await this.indexSettled(next, safeHead)
          : await this.indexUnsettled(next);

      return result;
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

  /** Settled blocks: a wide range, no inspection, one header read to anchor it. */
  private async indexSettled(from: number, safeHead: number): Promise<IterationResult> {
    const to = Math.min(from + this.effectiveMaxRange - 1, safeHead);

    const failure = await this.dispatch((p, signal) => p.onBlockRange(from, to, signal));
    if (failure) return failure;

    // Read even though these blocks cannot reorg. It costs one call per range —
    // ~94 across the entire backfill — and it is what lets `Cursor.lastHash`
    // mean the same thing always, so the detector can re-anchor from the cursor
    // alone when the loop crosses into the unsettled range or restarts.
    const header = await this.chain.getBlockHeader(to);
    await this.detector.commit(header);
    await this.saveCursor(header.number, header.hash);

    return { kind: 'progressed', cursorAt: to };
  }

  /** Unsettled blocks: one at a time, and the reorg question asked first. */
  private async indexUnsettled(next: number): Promise<IterationResult> {
    const header = await this.chain.getBlockHeader(next);
    const verdict = await this.detector.inspect(header);

    if (verdict.type === 'unrecoverable') {
      return { kind: 'failed', reason: `reorg beyond the detector's reach: ${verdict.reason}` };
    }

    if (verdict.type === 'reorg') {
      this.logger.warn(
        `reorg at block ${next}: blocks ${verdict.firstInvalidBlock}..${verdict.lastInvalidBlock} are stale`,
      );
      return this.applyReorg(
        verdict.firstInvalidBlock,
        verdict.lastInvalidBlock,
        verdict.lastValidHash,
      );
    }

    const failure = await this.dispatch((p, signal) => p.onBlockRange(next, next, signal));
    if (failure) return failure;

    await this.detector.commit(header);
    await this.saveCursor(header.number, header.hash);

    return { kind: 'progressed', cursorAt: next };
  }

  /**
   * Unwinds a fork. Nothing is mutated until every processor has accepted the
   * reorg, so a failure here leaves the rewind entirely unapplied rather than
   * half-done — the cursor still points into the abandoned branch and the whole
   * step is retried.
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
    await this.detector.rewindTo(lastValidBlock);
    await this.saveCursor(lastValidBlock, lastValidHash);

    return { kind: 'progressed', cursorAt: lastValidBlock };
  }

  /**
   * Runs the processors in registration order, stopping at the first non-`ok`.
   * Returns `null` when all succeeded.
   *
   * Sequential rather than concurrent: ordering has to be reproducible for the
   * retry semantics to be testable, and fanning N processors' RPC calls out at
   * once is exactly what the measured provider rate limits punish. Fail-fast
   * means an earlier processor is re-invoked when a later one asks to retry,
   * which is why the interface requires idempotence.
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
        // Sequential on purpose — see the doc comment. Running these
        // concurrently would make the ordering unreproducible and fan every
        // processor's RPC calls out at once.
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
        this.logger.warn(`iteration failed (attempt ${this.attempts}): ${result.reason}`);
      }

      // oxlint-disable-next-line no-await-in-loop
      await this.sleep(this.delayFor(result));
    }

    this.state = 'stopped';
  }

  private delayFor(result: IterationResult): number {
    if (result.kind === 'progressed') return 0;
    if (result.kind === 'idle') return this.options.pollIntervalMs;

    const exponent = Math.min(this.attempts - 1, 16);
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
