import { metrics } from '@opentelemetry/api';
import {
  AggregationTemporality,
  InMemoryMetricExporter,
  MeterProvider,
  PeriodicExportingMetricReader,
  type DataPoint,
  type ResourceMetrics,
} from '@opentelemetry/sdk-metrics';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { IndexerMetrics } from './indexer-metrics';
import { IndexerStatus } from './indexer-status';
import type { IndexingOptions } from '../indexing.options';

const OPTIONS = {
  chainId: 1,
  rpcUrls: ['https://rpc.invalid'],
  rpcTimeoutMs: 1_000,
  finalityDepth: 128,
  startBlock: 0,
  maxRangeSize: 1_000,
  pollIntervalMs: 4_000,
  stallThresholdMs: 300_000,
  autoStart: false,
} satisfies IndexingOptions;

let exporter: InMemoryMetricExporter;
let reader: PeriodicExportingMetricReader;
let provider: MeterProvider;
let status: IndexerStatus;

beforeEach(() => {
  exporter = new InMemoryMetricExporter(AggregationTemporality.CUMULATIVE);
  reader = new PeriodicExportingMetricReader({
    exporter,
    // Long enough that nothing exports on its own; every collection in these
    // specs is a deliberate `forceFlush`, so a timer cannot make them flaky.
    exportIntervalMillis: 600_000,
  });
  provider = new MeterProvider({ readers: [reader] });
  metrics.setGlobalMeterProvider(provider);
  status = new IndexerStatus();
});

afterEach(async () => {
  await provider.shutdown();
  metrics.disable();
});

/** The last export's data points for one instrument, across every scope. */
async function collect(name: string): Promise<DataPoint<number>[]> {
  await reader.forceFlush();
  const batches: ResourceMetrics[] = exporter.getMetrics();
  const latest = batches.at(-1);
  if (latest === undefined) return [];

  return latest.scopeMetrics
    .flatMap((scope) => scope.metrics)
    .filter((metric) => metric.descriptor.name === name)
    .flatMap((metric) => metric.dataPoints as DataPoint<number>[]);
}

/**
 * Constructing it is the registration: the batch callback that feeds every
 * gauge is installed by the constructor, so these specs never touch the
 * instance afterwards.
 */
function register(subject: IndexerStatus): IndexerMetrics {
  return new IndexerMetrics(subject, OPTIONS);
}

async function valueOf(name: string): Promise<number | undefined> {
  return (await collect(name))[0]?.value;
}

describe('IndexerMetrics', () => {
  describe('the lag gauge', () => {
    it('reports head minus cursor', async () => {
      register(status);
      status.observeHead(130);
      status.progressed(100);

      expect(await valueOf('indexer.lag.blocks')).toBe(30);
      expect(await valueOf('indexer.cursor.block')).toBe(100);
      expect(await valueOf('indexer.head.block')).toBe(130);
    });

    it('reads at collection time, so a stalled loop keeps reporting a growing lag', async () => {
      register(status);
      status.observeHead(130);
      status.progressed(100);
      expect(await valueOf('indexer.lag.blocks')).toBe(30);

      // The chain moves on. The loop does not — no transition happens, which is
      // exactly what a stall is. A gauge pushed from the transition would still
      // be reporting 30 here, which is the failure this design exists to avoid.
      status.observeHead(200);

      expect(await valueOf('indexer.lag.blocks')).toBe(100);
    });

    it('reports nothing at all before the first block, rather than zero', async () => {
      register(status);

      // Zero would draw a cliff on the graph at every restart, and briefly make
      // the lag look like the whole chain.
      expect(await collect('indexer.lag.blocks')).toHaveLength(0);
      expect(await collect('indexer.cursor.block')).toHaveLength(0);
    });

    it('never goes negative when the head is behind the cursor', async () => {
      register(status);
      status.observeHead(100);
      // A rewind after a reorg moves the cursor back, but a provider failover
      // can also leave the clamped head momentarily below it.
      status.progressed(120);

      expect(await valueOf('indexer.lag.blocks')).toBe(0);
    });
  });

  it('reads the snapshot once per collection, so the gauges cannot disagree', async () => {
    register(status);
    status.observeHead(130);
    status.progressed(100);

    const snapshot = vi.spyOn(status, 'snapshot', 'get');
    await reader.forceFlush();

    // Seven gauges, one read. Three separate callbacks reading a moving loop
    // could report a cursor and a head from two different moments, and the lag
    // computed from neither.
    expect(snapshot).toHaveBeenCalledTimes(1);
  });

  it('reports the state as one series per state, so failed is alertable without parsing a string', async () => {
    register(status);
    status.failed('chain id mismatch');

    const points = await collect('indexer.state');
    const byState = new Map(points.map((p) => [p.attributes['state'], p.value]));

    expect(byState.get('failed')).toBe(1);
    expect(byState.get('running')).toBe(0);
    expect(byState.get('starting')).toBe(0);
  });

  it('counts iterations by outcome', async () => {
    const subject = new IndexerMetrics(status, OPTIONS);
    subject.iteration({ kind: 'progressed', cursorAt: 10 }, 12);
    subject.iteration({ kind: 'retry', reason: 'boom' }, 3);
    subject.iteration({ kind: 'retry', reason: 'boom again' }, 4);

    const byOutcome = new Map(
      (await collect('indexer.iterations')).map((p) => [p.attributes['outcome'], p.value]),
    );

    expect(byOutcome.get('progressed')).toBe(1);
    expect(byOutcome.get('retry')).toBe(2);
  });

  it('reports the range size, so a provider narrowing us is visible', async () => {
    const subject = new IndexerMetrics(status, OPTIONS);
    expect(await valueOf('indexer.range.size')).toBe(1_000);

    subject.narrowedTo(500);

    expect(await valueOf('indexer.range.size')).toBe(500);
  });
});
