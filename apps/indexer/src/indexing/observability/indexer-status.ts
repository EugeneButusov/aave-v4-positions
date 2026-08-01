import { Injectable, Logger } from '@nestjs/common';

export type IndexerState = 'starting' | 'running' | 'retrying' | 'failed' | 'stopped';

export interface IndexerSnapshot {
  readonly state: IndexerState;
  /** Why the loop last failed to progress; cleared on the next success. */
  readonly reason: string | null;
  /** Cursor position, or `null` before anything has been indexed. */
  readonly lastBlock: number | null;
  /** Highest head observed, or `null` before the chain has been read. */
  readonly head: number | null;
  readonly lastProgressAt: number;
  readonly consecutiveFailures: number;
}

/**
 * Everything the outside world can learn about the indexing loop, and the state
 * machine behind it.
 *
 * Not purely a reporting surface, and it would be dishonest to name it as one:
 * the loop reads {@link isFailed} to know it is finished and
 * {@link consecutiveFailures} to size its backoff. Separating "the facts" from
 * "the facts we report" would mean keeping the same values in two places.
 *
 * What the split does buy is a health indicator that depends on this rather than
 * on the whole engine, a state machine that can be driven and asserted without
 * standing up a chain, and an {@link IndexerService} left holding only the work.
 */
@Injectable()
export class IndexerStatus {
  private readonly logger = new Logger(IndexerStatus.name);

  private state: IndexerState = 'starting';
  private reason: string | null = null;
  private lastBlock: number | null = null;
  private head = -1;
  private lastProgressAt = Date.now();
  private failures = 0;

  get snapshot(): IndexerSnapshot {
    return {
      state: this.state,
      reason: this.reason,
      lastBlock: this.lastBlock,
      head: this.head < 0 ? null : this.head,
      lastProgressAt: this.lastProgressAt,
      consecutiveFailures: this.failures,
    };
  }

  get isFailed(): boolean {
    return this.state === 'failed';
  }

  get failureReason(): string | null {
    return this.reason;
  }

  get consecutiveFailures(): number {
    return this.failures;
  }

  // ---------------------------------------------------------------------------
  // An observation, made part-way through an iteration and answered immediately.
  // Distinct from the transitions below, which are recorded once the iteration
  // has an outcome.
  // ---------------------------------------------------------------------------

  /**
   * Records an observed head and returns the value the loop should act on.
   *
   * Clamped to the high-water mark. Providers are not synchronised and viem's
   * `fallback` does not reconcile them, so failing over to a lagging node makes
   * the head appear to move backwards — taken at face value that is
   * indistinguishable from a reorg, and it can invert a range.
   *
   * Deliberately not folded into {@link progressed}. The head is read on every
   * iteration, including ones that go on to idle, retry or fail, and the loop
   * needs the clamped value back before it knows which of those it will be.
   * Updating it only on progress would freeze the reported head whenever the
   * loop stopped advancing — exactly when someone is reading it — so a stuck
   * indexer would understate its own lag.
   */
  observeHead(observed: number): number {
    if (observed < this.head) {
      this.logger.warn(`head regressed ${this.head} -> ${observed}; clamping to the high mark`);
    }
    this.head = Math.max(this.head, observed);
    return this.head;
  }

  // ---------------------------------------------------------------------------
  // Transitions. One per iteration, recorded from its result.
  // ---------------------------------------------------------------------------

  progressed(cursorAt: number): void {
    this.lastBlock = cursorAt;
    this.advanced();
  }

  /**
   * Caught up to the head with nothing to do. Counts as progress: being current
   * is the healthy steady state, and a quiet chain must not trip the stall alarm.
   */
  idled(): void {
    this.advanced();
  }

  retried(reason: string): void {
    this.state = 'retrying';
    this.reason = reason;
    this.failures += 1;
  }

  /** Terminal. Nothing here clears it; a process restart does. */
  failed(reason: string): void {
    this.state = 'failed';
    this.reason = reason;
  }

  stopped(): void {
    this.state = 'stopped';
  }

  private advanced(): void {
    this.state = 'running';
    this.reason = null;
    this.failures = 0;
    this.lastProgressAt = Date.now();
  }
}
