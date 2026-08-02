import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest';

import { FakeChainClient, hashOf } from '../test-support/fake-chain-client';
import { RecordingCursorStore } from '../test-support/recording-cursor-store';
import { RecordingProcessor } from '../test-support/recording-processor';
import { ScriptedReorgDetector } from '../test-support/scripted-reorg-detector';
import { failed, retry } from './processors/block-processor';
import type { Cursor } from './cursor/cursor-store';
import { IndexerService } from './indexer.service';
import * as otel from '@opentelemetry/api';
import { AsyncLocalStorageContextManager } from '@opentelemetry/context-async-hooks';
import {
  BasicTracerProvider,
  InMemorySpanExporter,
  SimpleSpanProcessor,
} from '@opentelemetry/sdk-trace-base';

import { IndexerMetrics } from './observability/indexer-metrics';
import { IndexerStatus } from './observability/indexer-status';
import type { IndexingOptions } from './indexing.options';

const BASE_OPTIONS: IndexingOptions = {
  chainId: 1,
  rpcUrls: ['https://rpc.invalid'],
  rpcTimeoutMs: 1_000,
  finalityDepth: 10,
  startBlock: 100,
  maxRangeSize: 50,
  pollIntervalMs: 5,
  stallThresholdMs: 60_000,
  autoStart: false,
};

interface Harness {
  readonly service: IndexerService;
  readonly status: IndexerStatus;
  readonly chain: FakeChainClient;
  readonly detector: ScriptedReorgDetector;
  readonly store: RecordingCursorStore;
  readonly processors: RecordingProcessor[];
  /** Interleaved detector and processor calls, for order assertions. */
  readonly trace: string[];
}

function harness(
  setup: {
    options?: Partial<IndexingOptions>;
    chain?: FakeChainClient;
    detector?: ScriptedReorgDetector;
    store?: RecordingCursorStore;
    processorCount?: number;
    processors?: RecordingProcessor[];
  } = {},
): Harness {
  const trace: string[] = [];
  const chain = setup.chain ?? new FakeChainClient({ head: 1_000 });
  const detector = setup.detector ?? new ScriptedReorgDetector(trace);
  const store = setup.store ?? new RecordingCursorStore();
  const processors =
    setup.processors ??
    Array.from(
      { length: setup.processorCount ?? 1 },
      (_, i) => new RecordingProcessor(`p${i + 1}`, trace),
    );

  const status = new IndexerStatus();
  const service = new IndexerService(
    { ...BASE_OPTIONS, ...setup.options },
    chain,
    detector,
    store,
    status,
    new IndexerMetrics(status, { ...BASE_OPTIONS, ...setup.options }),
    processors,
  );

  return { service, status, chain, detector, store, processors, trace };
}

function cursorAt(lastBlock: number, branch = 'a'): Cursor {
  return { chainId: 1, lastBlock, lastHash: hashOf(branch, lastBlock) };
}

/** Every range this processor was asked to handle, as `[from, to]` pairs. */
function ranges(processor: RecordingProcessor): [number, number][] {
  return processor.calls
    .filter((call) => call.kind === 'range')
    .map((call) => [call.from, call.to]);
}

describe('IndexerService — processors', () => {
  it('advances with no processors registered', async () => {
    const { service, store } = harness({ processors: [] });

    await expect(service.runOnce()).resolves.toMatchObject({ kind: 'progressed' });
    expect(store.saved).toHaveLength(1);
  });

  it('runs every processor over the same range, in registration order', async () => {
    const { service, detector, trace, processors } = harness({ processorCount: 3 });
    detector.settledThrough(1_000);

    await service.runOnce();

    expect(trace.filter((t) => t.includes('onBlockRange'))).toEqual([
      'p1.onBlockRange(100,149)',
      'p2.onBlockRange(100,149)',
      'p3.onBlockRange(100,149)',
    ]);
    for (const processor of processors) expect(ranges(processor)).toEqual([[100, 149]]);
  });

  it('stops at the first processor that does not succeed', async () => {
    const { service, detector, store, processors } = harness({ processorCount: 3 });
    detector.settledThrough(1_000);
    processors[1]?.queue(retry('backend busy'));

    const result = await service.runOnce();

    expect(result).toEqual({ kind: 'retry', reason: 'p2: backend busy' });
    // Fail-fast: p3 must not have been invoked, and nothing was committed.
    expect(processors[2]?.calls).toEqual([]);
    expect(store.saved).toEqual([]);
  });

  it('names the processor that failed, so a log line is actionable', async () => {
    const { service, detector, processors } = harness({ processorCount: 2 });
    detector.settledThrough(1_000);
    processors[1]?.queue(failed('unparseable event schema'));

    await expect(service.runOnce()).resolves.toEqual({
      kind: 'failed',
      reason: 'p2: unparseable event schema',
    });
  });
});

describe('IndexerService — dispatch shape', () => {
  it('dispatches a full-width range while catching up', async () => {
    const { service, detector, processors } = harness({ options: { maxRangeSize: 200 } });
    detector.settledThrough(1_000);

    await service.runOnce();

    expect(ranges(processors[0]!)).toEqual([[100, 299]]);
  });

  it('dispatches one block at a time near the tip', async () => {
    const { service, processors } = harness({ chain: new FakeChainClient({ head: 105 }) });

    await service.runOnce();

    expect(ranges(processors[0]!)).toEqual([[100, 100]]);
  });

  it('never lets a range cross the settled boundary', async () => {
    const { service, detector, processors } = harness({ options: { maxRangeSize: 500 } });
    // Everything up to 250 is settled; a 500-wide range would run to 599.
    detector.settledThrough(250);

    await service.runOnce();

    expect(ranges(processors[0]!)).toEqual([[100, 250]]);
  });

  it('does not touch the reorg path when the chain is continuous', async () => {
    const { service, processors } = harness({ chain: new FakeChainClient({ head: 105 }) });

    await service.runOnce();

    expect(processors[0]?.calls.filter((c) => c.kind === 'reorg')).toEqual([]);
  });

  it('does not dispatch blocks in the same iteration as a reorg', async () => {
    const { service, detector, processors } = harness({
      chain: new FakeChainClient({ head: 105 }),
    });
    detector.queue({
      type: 'reorg',
      firstInvalidBlock: 98,
      lastInvalidBlock: 99,
      lastValidHash: hashOf('a', 97),
    });

    await service.runOnce();

    expect(processors[0]?.calls).toEqual([{ kind: 'reorg', from: 98, to: 99, aborted: false }]);
  });
});

describe('IndexerService — resume', () => {
  it('bootstraps the detector once, before reading the head', async () => {
    const { service, chain, detector } = harness();

    await service.runOnce();
    await service.runOnce();

    expect(detector.bootstrappedWith).toHaveLength(1);
    // The detector must be primed before any indexing decision is taken.
    expect(chain.calls[0]).toEqual({ method: 'getChainId' });
    expect(chain.callsTo('getHeadBlockNumber')).toHaveLength(2);
  });

  it('resumes from the stored cursor rather than the start block or the tip', async () => {
    const { service, detector, processors } = harness({
      store: new RecordingCursorStore(cursorAt(500)),
    });
    detector.settledThrough(1_000);

    await service.runOnce();

    // Not 100 (the configured start) and not 1000 (the tip).
    expect(ranges(processors[0]!)).toEqual([[501, 550]]);
  });

  it('starts at the configured block when nothing is stored', async () => {
    const { service, detector, processors } = harness();
    detector.settledThrough(1_000);

    await service.runOnce();

    expect(ranges(processors[0]!)).toEqual([[100, 149]]);
  });

  it('prefers the stored cursor even when it sits below the start block', async () => {
    const { service, detector, processors } = harness({
      store: new RecordingCursorStore(cursorAt(40)),
      options: { startBlock: 100 },
    });
    detector.settledThrough(1_000);

    await service.runOnce();

    expect(ranges(processors[0]!)).toEqual([[41, 90]]);
  });

  it('unwinds a fork that happened while the process was down', async () => {
    const { service, detector, store, processors } = harness({
      store: new RecordingCursorStore(cursorAt(500)),
    });
    detector.onBootstrap({
      type: 'reorg',
      firstInvalidBlock: 495,
      lastInvalidBlock: 500,
      lastValidHash: hashOf('a', 494),
    });

    const result = await service.runOnce();

    // Without this the loop would carry on from 501 onto the branch that lost.
    expect(result).toEqual({ kind: 'progressed', cursorAt: 494 });
    expect(processors[0]?.calls).toEqual([{ kind: 'reorg', from: 495, to: 500, aborted: false }]);
    expect(store.saved).toEqual([cursorAt(494)]);
  });

  it('refuses to resume when the detector cannot place the cursor', async () => {
    const { service, detector, store, processors } = harness({
      store: new RecordingCursorStore(cursorAt(500)),
    });
    detector.onBootstrap({ type: 'unrecoverable', reason: 'cursor predates the window' });

    const result = await service.runOnce();

    expect(result).toEqual({
      kind: 'failed',
      reason: 'cannot resume: cursor predates the window',
    });
    expect(processors[0]?.calls).toEqual([]);
    expect(store.saved).toEqual([]);
  });

  it('retries the whole bootstrap when the reorg dispatch fails', async () => {
    const { service, detector, processors } = harness({
      store: new RecordingCursorStore(cursorAt(500)),
    });
    detector.onBootstrap({
      type: 'reorg',
      firstInvalidBlock: 495,
      lastInvalidBlock: 500,
      lastValidHash: hashOf('a', 494),
    });
    processors[0]?.queue(retry('store locked'));

    await service.runOnce();
    await service.runOnce();

    // Bootstrap is not marked done until it has fully succeeded, or the
    // detector would be left half-primed.
    expect(detector.bootstrappedWith).toHaveLength(2);
  });
});

describe('IndexerService — outcome gates the cursor', () => {
  it('saves the cursor with the hash of the range top', async () => {
    const { service, detector, store } = harness();
    detector.settledThrough(1_000);

    await service.runOnce();

    expect(store.saved).toEqual([cursorAt(149)]);
  });

  it('holds position and re-dispatches the identical range on retry', async () => {
    const { service, detector, store, processors } = harness();
    detector.settledThrough(1_000);
    processors[0]?.queue(retry('rpc timeout'));

    await service.runOnce();
    await service.runOnce();

    expect(store.saved).toHaveLength(1);
    expect(ranges(processors[0]!)).toEqual([
      [100, 149],
      [100, 149],
    ]);
  });

  it('does no further work once a processor has failed', async () => {
    const { service, status, detector, store, processors } = harness();
    detector.settledThrough(1_000);
    processors[0]?.queue(failed('schema mismatch'));

    await service.runOnce();
    const second = await service.runOnce();

    expect(second.kind).toBe('failed');
    expect(ranges(processors[0]!)).toHaveLength(1);
    expect(store.saved).toEqual([]);
    expect(status.snapshot.state).toBe('failed');
  });

  it('treats a thrown error as retryable and keeps going', async () => {
    const { service, status, detector, store, processors } = harness();
    detector.settledThrough(1_000);
    processors[0]?.throwOnCall(new TypeError('cannot read property of undefined'));

    const result = await service.runOnce();

    expect(result).toEqual({
      kind: 'retry',
      reason: 'p1 threw: cannot read property of undefined',
    });
    expect(store.saved).toEqual([]);
    expect(status.snapshot.state).toBe('retrying');
  });

  it('halves the range when a processor asks to narrow', async () => {
    const { service, detector, processors } = harness({ options: { maxRangeSize: 200 } });
    detector.settledThrough(1_000);
    processors[0]?.queue(retry('range too large', { narrowRange: true }));

    await service.runOnce();
    await service.runOnce();

    expect(ranges(processors[0]!)).toEqual([
      [100, 299],
      [100, 199],
    ]);
  });

  it('re-dispatches the range when the cursor store rejects the write', async () => {
    const { service, detector, store, processors } = harness({
      store: new RecordingCursorStore().failSave(1),
    });
    detector.settledThrough(1_000);

    await service.runOnce();
    await service.runOnce();

    // The cursor is the commit point: an unwritten cursor means the range did
    // not happen, however much work the processors did.
    expect(ranges(processors[0]!)).toEqual([
      [100, 149],
      [100, 149],
    ]);
    expect(store.saved).toEqual([cursorAt(149)]);
  });
});

describe('IndexerService — the detector owns the chain shape', () => {
  it('takes the range path for everything when the detector settles the head', async () => {
    const { service, detector, processors } = harness();
    detector.settledThrough(1_000);

    await service.runOnce();

    // No finality arithmetic in the service: the detector's answer decides how
    // wide to dispatch, and settling the whole chain makes that one range.
    expect(ranges(processors[0]!)).toEqual([[100, 149]]);
  });

  it('takes the inspected path for everything when the detector settles nothing', async () => {
    const { service, detector, processors } = harness();

    await service.runOnce();
    await service.runOnce();

    expect(detector.inspected).toEqual([100, 101]);
    expect(ranges(processors[0]!)).toEqual([
      [100, 100],
      [101, 101],
    ]);
  });

  it('asks about the reorg before dispatching the block', async () => {
    const { service, trace } = harness({ chain: new FakeChainClient({ head: 105 }) });

    await service.runOnce();

    expect(trace).toEqual([
      'detector.bootstrap',
      'detector.inspect(100)',
      'p1.onBlockRange(100,100)',
      'detector.commit(100)',
    ]);
  });

  it('asks about a settled range once, not once per block', async () => {
    const { service, detector } = harness();
    detector.settledThrough(1_000);

    await service.runOnce();
    await service.runOnce();

    // Every header the loop commits is put to the detector first — but a range
    // yields one header for fifty blocks, so this is two questions rather than
    // a hundred. Whether a settled top needs checking is the detector's call;
    // the loop does not make it on its behalf.
    expect(detector.inspected).toEqual([149, 199]);
  });

  it('unwinds a fork in order: processors, then cursor, then detector', async () => {
    const { service, detector, store, trace } = harness({
      chain: new FakeChainClient({ head: 105 }),
      store: new RecordingCursorStore(cursorAt(102)),
    });
    detector.queue({
      type: 'reorg',
      firstInvalidBlock: 101,
      lastInvalidBlock: 102,
      lastValidHash: hashOf('a', 100),
    });

    const result = await service.runOnce();

    expect(result).toEqual({ kind: 'progressed', cursorAt: 100 });
    expect(trace).toEqual([
      'detector.bootstrap',
      'detector.inspect(103)',
      'p1.onReorg(101,102)',
      'detector.rewindTo(100)',
    ]);
    expect(store.saved).toEqual([cursorAt(100)]);
  });

  it('leaves the rewind entirely unapplied when a processor cannot accept it', async () => {
    const { service, detector, store, processors } = harness({
      chain: new FakeChainClient({ head: 105 }),
      store: new RecordingCursorStore(cursorAt(102)),
    });
    detector.queue({
      type: 'reorg',
      firstInvalidBlock: 101,
      lastInvalidBlock: 102,
      lastValidHash: hashOf('a', 100),
    });
    processors[0]?.queue(retry('rollback deadlocked'));

    await service.runOnce();

    // A half-applied rewind — detector rewound, cursor not, or vice versa —
    // would be worse than none at all.
    expect(detector.rewoundTo).toEqual([]);
    expect(store.saved).toEqual([]);
  });

  it('keeps the retained headers when the cursor save rejects', async () => {
    const { service, detector, store } = harness({
      chain: new FakeChainClient({ head: 105 }),
      store: new RecordingCursorStore(cursorAt(102)).failSave(1),
    });
    detector.queue({
      type: 'reorg',
      firstInvalidBlock: 101,
      lastInvalidBlock: 102,
      lastValidHash: hashOf('a', 100),
    });

    await service.runOnce();

    // Those headers are the evidence of the branch being abandoned, and the
    // rewind is what throws them away. Discarding them for a write that then
    // failed would leave the detector holding less than it started with, having
    // committed to nothing — so the rewind waits until the cursor is durable.
    expect(store.saved).toEqual([]);
    expect(detector.rewoundTo).toEqual([]);
  });

  it('stops rather than guessing when a fork runs deeper than the detector can see', async () => {
    const { service, detector, processors } = harness({
      chain: new FakeChainClient({ head: 105 }),
    });
    detector.queue({ type: 'unrecoverable', reason: 'no common ancestor within 128 blocks' });

    const result = await service.runOnce();

    expect(result).toEqual({
      kind: 'failed',
      reason: "reorg beyond the detector's reach: no common ancestor within 128 blocks",
    });
    expect(detector.rewoundTo).toEqual([]);
    expect(processors[0]?.calls).toEqual([]);
  });

  it('anchors the settled range so the first inspected block has a parent to check', async () => {
    const { service, detector, processors } = harness({
      chain: new FakeChainClient({ head: 160 }),
      // Settles through 149, so block 150 is the first inspected one.
      detector: new ScriptedReorgDetector().settledBelowDepth(11),
      options: { maxRangeSize: 50 },
    });

    // Catch-up commits the header at the top of its range, even though these
    // blocks cannot reorg and the commit is not needed for their own sake...
    await service.runOnce();
    expect(ranges(processors[0]!)).toEqual([[100, 149]]);
    expect(detector.committedHeaders.map((h) => h.number)).toEqual([149]);

    await service.runOnce();

    // 149 is asked about as the range's own top, then 150 as the first block
    // past the boundary.
    expect(detector.inspectedHeaders.map((h) => h.number)).toEqual([149, 150]);
    // ...because it is exactly the ancestry the first block past the boundary
    // is checked against. Without it the detector crosses the boundary blind.
    expect(detector.inspectedHeaders[1]?.parentHash).toBe(detector.committedHeaders[0]?.hash);
  });
});

describe('IndexerService — loop guards', () => {
  it('idles without dispatching when the start block is ahead of the chain', async () => {
    const { service, store, processors } = harness({
      chain: new FakeChainClient({ head: 50 }),
      options: { startBlock: 100 },
    });

    await expect(service.runOnce()).resolves.toEqual({ kind: 'idle' });
    expect(processors[0]?.calls).toEqual([]);
    expect(store.saved).toEqual([]);
  });

  it('idles once it has caught up to the head', async () => {
    const { service, detector } = harness({ chain: new FakeChainClient({ head: 100 }) });
    detector.settledThrough(1_000);

    await service.runOnce();

    await expect(service.runOnce()).resolves.toEqual({ kind: 'idle' });
  });

  it('clamps a head that goes backwards when a provider lags', async () => {
    const chain = new FakeChainClient({ head: 1_000 });
    const { service, status, detector, processors } = harness({ chain });
    detector.settledThrough(1_000);

    await service.runOnce();
    // A viem fallback failover can land on a node 15 blocks behind. Untreated,
    // this produces an inverted range or a spurious reorg.
    chain.setHead(940);
    await service.runOnce();

    expect(ranges(processors[0]!)).toEqual([
      [100, 149],
      [150, 199],
    ]);
    expect(status.snapshot.head).toBe(1_000);
  });

  it('stops when the providers serve a different chain than configured', async () => {
    const { service, processors } = harness({
      chain: new FakeChainClient({ chainId: 137, head: 1_000 }),
    });

    await expect(service.runOnce()).resolves.toEqual({
      kind: 'failed',
      reason: 'chain id mismatch: configured 1, provider reports 137',
    });
    expect(processors[0]?.calls).toEqual([]);
  });

  it('verifies the chain id once, not on every iteration', async () => {
    const { service, chain, detector } = harness();
    detector.settledThrough(1_000);

    await service.runOnce();
    await service.runOnce();
    await service.runOnce();

    expect(chain.callsTo('getChainId')).toHaveLength(1);
  });

  it('retries rather than stopping when the chain is unreachable', async () => {
    const { service, status, chain } = harness();
    chain.failNext(1, new Error('ECONNREFUSED'));

    const result = await service.runOnce();

    expect(result).toEqual({ kind: 'retry', reason: 'unhandled: ECONNREFUSED' });
    expect(status.snapshot.state).toBe('retrying');
  });

  it('does not start a loop when autostart is off', async () => {
    const { service, chain } = harness({ options: { autoStart: false } });

    service.onApplicationBootstrap();
    await Promise.resolve();

    expect(chain.calls).toEqual([]);
  });

  it('runs at most one loop however many times it is started', async () => {
    const { service, chain, detector } = harness({ options: { autoStart: true } });
    detector.settledThrough(1_000);

    service.onApplicationBootstrap();
    service.start();
    service.start();
    await service.onApplicationShutdown();

    // Two loops would race the same cursor. getChainId is verified once per
    // service, so more than one call means more than one loop.
    expect(chain.callsTo('getChainId')).toHaveLength(1);
  });

  it('stops the loop on shutdown', async () => {
    const { service, status, detector } = harness({ options: { autoStart: true } });
    detector.settledThrough(1_000);

    service.onApplicationBootstrap();
    await service.onApplicationShutdown();
    const afterShutdown = status.snapshot.lastBlock;

    await new Promise((resolve) => setTimeout(resolve, 20));

    expect(status.snapshot.state).toBe('stopped');
    expect(status.snapshot.lastBlock).toBe(afterShutdown);
  });

  it('does not sit out the poll interval before shutting down', async () => {
    const { service } = harness({
      // Start block above the head, so the first iteration idles and sleeps.
      chain: new FakeChainClient({ head: 100 }),
      options: { autoStart: true, startBlock: 101, pollIntervalMs: 5_000 },
    });

    service.onApplicationBootstrap();
    await new Promise((resolve) => setTimeout(resolve, 20));

    const startedAt = Date.now();
    await service.onApplicationShutdown();

    // The sleep wakes on the abort signal rather than expiring, so a deploy
    // does not pay the poll interval on every pod.
    expect(Date.now() - startedAt).toBeLessThan(500);
  });

  it('tells processors that shutdown has begun', async () => {
    const { service, detector, processors } = harness({ options: { autoStart: true } });
    detector.settledThrough(1_000);

    service.onApplicationBootstrap();
    await service.onApplicationShutdown();

    const aborted = processors[0]?.calls.some((call) => call.aborted) ?? false;
    const finished = processors[0]?.calls.length ?? 0;
    // Either it drained before the signal fired, or the calls in flight saw it.
    expect(aborted || finished > 0).toBe(true);
  });
});

describe('what the loop reports', () => {
  const exporter = new InMemorySpanExporter();

  beforeAll(() => {
    // Without this the spans below are three unparented roots. `startActiveSpan`
    // only survives an `await` when a context manager is installed, which
    // `NodeTracerProvider.register()` does in the running services and a bare
    // `BasicTracerProvider` does not — so a spec that omitted it would assert
    // the nesting away rather than prove it.
    otel.context.setGlobalContextManager(new AsyncLocalStorageContextManager().enable());
    otel.trace.setGlobalTracerProvider(
      new BasicTracerProvider({ spanProcessors: [new SimpleSpanProcessor(exporter)] }),
    );
  });

  afterEach(() => exporter.reset());
  afterAll(() => {
    otel.trace.disable();
    otel.context.disable();
  });

  /** `[name, parentName | null]` for every finished span, root first. */
  function tree(): [string, string | null][] {
    const spans = exporter.getFinishedSpans();
    const byId = new Map(spans.map((s) => [s.spanContext().spanId, s.name]));
    return spans.map((s) => [
      s.name,
      s.parentSpanContext === undefined ? null : (byId.get(s.parentSpanContext.spanId) ?? null),
    ]);
  }

  it('nests each processor under the iteration that dispatched it', async () => {
    const { service } = harness({ processorCount: 2, options: { startBlock: 100 } });

    await service.runOnce();

    // The nesting is the point: without an active context the processor spans
    // would be three unrelated roots, and nothing would say which iteration
    // a slow processor belonged to.
    expect(tree()).toEqual([
      ['processor.p1', 'indexer.iterate'],
      ['processor.p2', 'indexer.iterate'],
      ['indexer.iterate', null],
    ]);
  });

  it('marks a retrying iteration an error, with the reason', async () => {
    const stopper = new RecordingProcessor('stopper', []);
    stopper.onBlockRange = () => retry('provider said no');
    const { service } = harness({ processors: [stopper], options: { startBlock: 100 } });

    await service.runOnce();

    const iteration = exporter.getFinishedSpans().find((s) => s.name === 'indexer.iterate');
    // A retry is routine to the loop and an error to a trace backend. That
    // second framing is what makes failing iterations filterable out of
    // thousands of healthy ones.
    expect(iteration?.status.code).toBe(otel.SpanStatusCode.ERROR);
    expect(iteration?.status.message).toContain('provider said no');
    expect(iteration?.attributes['indexer.outcome']).toBe('retry');
  });

  it('emits no span at all once the loop is terminally failed', async () => {
    const { service, status } = harness();
    status.failed('chain id mismatch');

    await service.runOnce();

    // A stopped loop answers "still stopped" every poll interval. Tracing that
    // fills a backend with nothing.
    expect(exporter.getFinishedSpans()).toHaveLength(0);
  });

  it('records a reorg as an event on the iteration, with its depth', async () => {
    const { service } = harness({
      detector: new ScriptedReorgDetector().queue({
        type: 'reorg',
        firstInvalidBlock: 98,
        lastInvalidBlock: 100,
        lastValidHash: hashOf('a', 97),
      }),
      options: { startBlock: 100 },
    });

    await service.runOnce();

    const iteration = exporter.getFinishedSpans().find((s) => s.name === 'indexer.iterate');
    const reorg = iteration?.events.find((e) => e.name === 'reorg');
    // An event and not a span of its own: a fork is something that happened
    // inside an iteration, not a phase beside it.
    expect(reorg?.attributes).toMatchObject({ depth: 3, phase: 'inspect' });
  });
});
