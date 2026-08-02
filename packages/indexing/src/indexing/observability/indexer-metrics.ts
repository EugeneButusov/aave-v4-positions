import { Inject, Injectable } from '@nestjs/common';
import { metrics, type Counter, type Histogram } from '@opentelemetry/api';

import { IndexerStatus, type IndexerState } from './indexer-status';
import { INDEXING_OPTIONS, type IndexingOptions } from '../indexing.options';
import type { IterationResult } from '../indexer.service';

/** Every state the gauge reports, so a missing one reads 0 rather than absent. */
const STATES: readonly IndexerState[] = ['starting', 'running', 'retrying', 'failed', 'stopped'];

/**
 * What the indexing loop reports about itself.
 *
 * The README used to propose a write-only `IndexerObserver` port here, to keep a
 * metrics vendor out of this package. `@opentelemetry/api` **is** that port: a
 * vendor-neutral facade that is a no-op until an SDK registers, one package with
 * no transitive dependencies. A port over a facade is the same indirection
 * twice, and tracing settles it either way — a span has to *wrap* the work, so
 * it cannot be delivered by an observer told what already happened.
 *
 * **The gauges are observable and read {@link IndexerStatus} at collection
 * time.** That is the load-bearing choice in this file. Pushing them from the
 * state transition would look equivalent and is not: a stalled loop makes no
 * transitions, so a pushed gauge freezes at its last value — reporting healthy
 * numbers precisely when someone is looking at it because it stopped. Reading
 * on collection means a wedged indexer's lag keeps climbing on the graph.
 *
 * Counters and histograms are the opposite case and are recorded where the
 * event happens, because there is no state to sample: an iteration that already
 * finished is not visible in any snapshot.
 */
@Injectable()
export class IndexerMetrics {
  private readonly iterations: Counter;
  private readonly iterationDuration: Histogram;
  private readonly processorDuration: Histogram;
  private readonly reorgDepth: Histogram;
  private readonly blocksIndexed: Counter;

  /**
   * Mirrors `IndexerService`'s `effectiveMaxRange`, which only ever halves. Held
   * here rather than read from the service because the service does not expose
   * it, and a getter added purely for a metric is the kind of API a metric
   * should not be allowed to grow.
   */
  private rangeSize: number;

  constructor(
    private readonly status: IndexerStatus,
    @Inject(INDEXING_OPTIONS) private readonly options: IndexingOptions,
  ) {
    const meter = metrics.getMeter('@packages/indexing');
    this.rangeSize = options.maxRangeSize;

    this.iterations = meter.createCounter('indexer.iterations', {
      description: 'Loop iterations by outcome. The retry rate, long before the stall alarm.',
    });
    this.iterationDuration = meter.createHistogram('indexer.iteration.duration', {
      unit: 's',
      description:
        'How long one pass took, so a slow iteration is distinguishable from a frequent one.',
    });
    this.processorDuration = meter.createHistogram('indexer.processor.duration', {
      unit: 's',
      description:
        'Per processor. Dispatch is sequential, so one slow processor is the whole loop.',
    });
    this.reorgDepth = meter.createHistogram('indexer.reorg.depth', {
      unit: '{block}',
      description:
        'How deep each reorg reached; its count is how often. Watch it against FINALITY_DEPTH.',
    });
    this.blocksIndexed = meter.createCounter('indexer.blocks.indexed', {
      unit: '{block}',
      description: 'Throughput. Its rate over the lag gauge is the catch-up estimate.',
    });

    this.observeStatus(meter);
  }

  /**
   * One batch callback for every gauge, rather than one callback each.
   *
   * `snapshot` is read **once** per collection, so cursor, head and lag can
   * never disagree with each other inside a single scrape — which they could if
   * three callbacks each read the status at three moments of a moving loop.
   */
  private observeStatus(meter: ReturnType<typeof metrics.getMeter>): void {
    const cursor = meter.createObservableGauge('indexer.cursor.block', {
      unit: '{block}',
      description: 'How far the loop has got, durably.',
    });
    const head = meter.createObservableGauge('indexer.head.block', {
      unit: '{block}',
      description: 'The highest head observed, clamped to a high-water mark.',
    });
    const lag = meter.createObservableGauge('indexer.lag.blocks', {
      unit: '{block}',
      description: 'Head minus cursor. The alert, and the number that had no way out before.',
    });
    const staleness = meter.createObservableGauge('indexer.progress.age', {
      unit: 's',
      description:
        'Seconds since anything advanced. Graphable before it crosses INDEXER_STALL_THRESHOLD_MS.',
    });
    const failures = meter.createObservableGauge('indexer.consecutive_failures', {
      unit: '{failure}',
      description: 'Sizes the backoff, so it also says how long until the next attempt.',
    });
    const state = meter.createObservableGauge('indexer.state', {
      description: 'One series per state, 1 or 0. Alert on failed without parsing an error string.',
    });
    const range = meter.createObservableGauge('indexer.range.size', {
      unit: '{block}',
      description:
        'Dispatch width, which only ever halves. A falling line is a provider degrading us.',
    });

    meter.addBatchObservableCallback(
      (observed) => {
        const snapshot = this.status.snapshot;
        const chain = { 'chain.id': this.options.chainId };

        // `lastBlock` is null before anything has been indexed, and `head` is
        // null before the chain has been read. Reporting 0 for either would
        // draw a cliff on the graph at every restart and make the lag briefly
        // enormous; reporting nothing draws a gap, which is what it is.
        if (snapshot.lastBlock !== null) observed.observe(cursor, snapshot.lastBlock, chain);
        if (snapshot.head !== null) observed.observe(head, snapshot.head, chain);
        if (snapshot.lastBlock !== null && snapshot.head !== null) {
          observed.observe(lag, Math.max(0, snapshot.head - snapshot.lastBlock), chain);
        }

        observed.observe(staleness, (Date.now() - snapshot.lastProgressAt) / 1000, chain);
        observed.observe(failures, snapshot.consecutiveFailures, chain);
        observed.observe(range, this.rangeSize, chain);

        for (const candidate of STATES) {
          observed.observe(state, snapshot.state === candidate ? 1 : 0, {
            ...chain,
            state: candidate,
          });
        }
      },
      [cursor, head, lag, staleness, failures, state, range],
    );
  }

  // ---------------------------------------------------------------------------
  // Recorded where the event happens, because no snapshot remembers it.
  // ---------------------------------------------------------------------------

  iteration(result: IterationResult, durationMs: number): void {
    const attributes = { 'chain.id': this.options.chainId, outcome: result.kind };
    this.iterations.add(1, attributes);
    this.iterationDuration.record(durationMs / 1000, attributes);
  }

  processor(name: string, operation: string, outcome: string, durationMs: number): void {
    this.processorDuration.record(durationMs / 1000, {
      'chain.id': this.options.chainId,
      processor: name,
      operation,
      outcome,
    });
  }

  reorg(depth: number, phase: 'bootstrap' | 'inspect'): void {
    this.reorgDepth.record(depth, { 'chain.id': this.options.chainId, phase });
  }

  indexed(blocks: number): void {
    this.blocksIndexed.add(blocks, { 'chain.id': this.options.chainId });
  }

  narrowedTo(size: number): void {
    this.rangeSize = size;
  }
}
