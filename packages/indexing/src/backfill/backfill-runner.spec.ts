import { describe, expect, it } from 'vitest';

import { FakeChainClient } from '../test-support/fake-chain-client';
import { RecordingProcessor } from '../test-support/recording-processor';
import { ScriptedReorgDetector } from '../test-support/scripted-reorg-detector';
import { failed, ok, retry, type BlockProcessor } from '../indexing/processors/block-processor';
import type { IndexingOptions } from '../indexing/indexing.options';
import { BackfillRunner, type BackfillRequest } from './backfill-runner';

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

const HEAD = 1_000;

interface Harness {
  readonly runner: BackfillRunner;
  readonly chain: FakeChainClient;
  readonly processors: RecordingProcessor[];
}

function harness(
  setup: {
    options?: Partial<IndexingOptions>;
    chain?: FakeChainClient;
    /** Everything up to and including this block counts as settled. */
    safeHead?: number;
    processorCount?: number;
    processors?: BlockProcessor[];
  } = {},
): Harness {
  const chain = setup.chain ?? new FakeChainClient({ head: HEAD });
  const detector = new ScriptedReorgDetector().settledThrough(setup.safeHead ?? HEAD);
  const processors =
    setup.processors ??
    Array.from(
      { length: setup.processorCount ?? 1 },
      (_, i) => new RecordingProcessor(`p${i + 1}`),
    );

  const runner = new BackfillRunner(
    { ...BASE_OPTIONS, ...setup.options },
    chain,
    detector,
    processors,
  );

  return { runner, chain, processors: processors.filter((p) => p instanceof RecordingProcessor) };
}

/** Every range this processor was asked to index, as `[from, to]` pairs. */
function ranges(processor: RecordingProcessor): [number, number][] {
  return processor.calls
    .filter((call) => call.kind === 'range')
    .map((call) => [call.from, call.to]);
}

function request(overrides: Partial<BackfillRequest> = {}): BackfillRequest {
  return { from: 100, to: 249, ...overrides };
}

const NEVER_ABORTS = new AbortController().signal;

describe('BackfillRunner', () => {
  describe('slicing', () => {
    it('splits the range into slices of the configured width', async () => {
      const { runner, processors } = harness();

      const result = await runner.run(request(), NEVER_ABORTS);

      expect(result).toEqual({ kind: 'completed', ranges: 3, blocks: 150 });
      expect(ranges(processors[0]!)).toEqual([
        [100, 149],
        [150, 199],
        [200, 249],
      ]);
    });

    it('leaves the last slice short rather than overshooting the requested range', async () => {
      const { runner, processors } = harness();

      await runner.run(request({ to: 219 }), NEVER_ABORTS);

      expect(ranges(processors[0]!).at(-1)).toEqual([200, 219]);
    });

    it('dispatches a single-block range as [n, n]', async () => {
      const { runner, processors } = harness();

      const result = await runner.run(request({ from: 500, to: 500 }), NEVER_ABORTS);

      expect(result).toEqual({ kind: 'completed', ranges: 1, blocks: 1 });
      expect(ranges(processors[0]!)).toEqual([[500, 500]]);
    });

    it('honours a per-run width override rather than the configured one', async () => {
      const { runner, processors } = harness();

      await runner.run(request({ to: 199, rangeSize: 25 }), NEVER_ABORTS);

      expect(ranges(processors[0]!)).toHaveLength(4);
    });

    it('refuses a range that runs backwards', async () => {
      const { runner, processors } = harness();

      const result = await runner.run(request({ from: 300, to: 200 }), NEVER_ABORTS);

      expect(result).toMatchObject({ kind: 'failed', resumeFrom: 300 });
      expect(processors[0]!.calls).toEqual([]);
    });
  });

  describe('processors', () => {
    it('gives every slice to every processor, in registration order', async () => {
      const { runner, processors } = harness({ processorCount: 2 });

      await runner.run(request(), NEVER_ABORTS);

      expect(ranges(processors[0]!)).toEqual(ranges(processors[1]!));
      expect(ranges(processors[1]!)).toHaveLength(3);
    });

    it('runs only the processors named', async () => {
      const { runner, processors } = harness({ processorCount: 3 });

      await runner.run(request({ processors: ['p2'] }), NEVER_ABORTS);

      expect(processors[0]!.calls).toEqual([]);
      expect(ranges(processors[1]!)).toHaveLength(3);
      expect(processors[2]!.calls).toEqual([]);
    });

    it('refuses an unknown processor name, listing what is registered', async () => {
      const { runner, processors } = harness({ processorCount: 2 });

      const result = await runner.run(request({ processors: ['nope'] }), NEVER_ABORTS);

      expect(result).toMatchObject({ kind: 'failed' });
      expect(result).toHaveProperty('reason', expect.stringContaining('nope'));
      expect(result).toHaveProperty('reason', expect.stringContaining('p1, p2'));
      expect(processors[0]!.calls).toEqual([]);
    });

    it('refuses to run with nothing registered rather than reporting a silent success', async () => {
      const { runner } = harness({ processors: [] });

      expect(await runner.run(request(), NEVER_ABORTS)).toMatchObject({ kind: 'failed' });
    });

    it('never asks a processor to discard — a backfill only ever moves forward', async () => {
      const { runner, processors } = harness();

      await runner.run(request(), NEVER_ABORTS);

      expect(processors[0]!.calls.every((call) => call.kind === 'range')).toBe(true);
    });
  });

  describe('the safe head', () => {
    it('refuses a range reaching above it, naming the highest allowed block', async () => {
      const { runner, processors } = harness({ safeHead: 900 });

      const result = await runner.run(request({ from: 800, to: 950 }), NEVER_ABORTS);

      expect(result).toMatchObject({ kind: 'failed', resumeFrom: 800 });
      expect(result).toHaveProperty('reason', expect.stringContaining('900'));
      expect(processors[0]!.calls).toEqual([]);
    });

    it('runs a range that ends exactly on it', async () => {
      const { runner } = harness({ safeHead: 900 });

      expect(await runner.run(request({ from: 851, to: 900 }), NEVER_ABORTS)).toMatchObject({
        kind: 'completed',
      });
    });
  });

  describe('failure', () => {
    it('refuses a chain id mismatch before dispatching anything', async () => {
      const chain = new FakeChainClient({ head: HEAD }).setChainId(137);
      const { runner, processors } = harness({ chain });

      const result = await runner.run(request(), NEVER_ABORTS);

      expect(result).toMatchObject({ kind: 'failed', resumeFrom: 100 });
      expect(result).toHaveProperty('reason', expect.stringContaining('137'));
      expect(processors[0]!.calls).toEqual([]);
    });

    it('stops on a failed outcome, reporting the slice that did not land', async () => {
      const processor = new RecordingProcessor('decoder').queue(ok(), failed('bad schema'));
      const { runner } = harness({ processors: [processor] });

      const result = await runner.run(request(), NEVER_ABORTS);

      expect(result).toMatchObject({ kind: 'failed', resumeFrom: 150 });
      expect(result).toHaveProperty('reason', expect.stringContaining('bad schema'));
    });

    it('gives up after the attempt limit, reporting where to resume', async () => {
      const processor = new RecordingProcessor('flaky').queue(ok(), retry('rpc down'));
      const { runner } = harness({ processors: [processor] });

      const result = await runner.run(request({ maxAttempts: 1 }), NEVER_ABORTS);

      expect(result).toMatchObject({ kind: 'failed', resumeFrom: 150 });
      expect(result).toHaveProperty('reason', expect.stringContaining('gave up'));
    });

    it('re-dispatches the same slice after a retry, then carries on', async () => {
      const processor = new RecordingProcessor('flaky').queue(retry('rpc down'));
      const { runner } = harness({ processors: [processor] });

      const result = await runner.run(request({ to: 199 }), NEVER_ABORTS);

      expect(result).toEqual({ kind: 'completed', ranges: 2, blocks: 100 });
      expect(ranges(processor)).toEqual([
        [100, 149],
        [100, 149],
        [150, 199],
      ]);
    });

    it('halves the width when asked to narrow, and keeps it narrowed', async () => {
      const processor = new RecordingProcessor('picky').queue(
        retry('range too wide', { narrowRange: true }),
      );
      const { runner } = harness({ processors: [processor] });

      await runner.run(request({ to: 199 }), NEVER_ABORTS);

      expect(ranges(processor)).toEqual([
        [100, 149],
        [100, 124],
        [125, 149],
        [150, 174],
        [175, 199],
      ]);
    });
  });

  describe('interruption', () => {
    it('dispatches nothing when the signal is already set', async () => {
      const { runner, processors } = harness();
      const aborted = AbortSignal.abort();

      const result = await runner.run(request(), aborted);

      expect(result).toEqual({ kind: 'aborted', resumeFrom: 100 });
      expect(processors[0]!.calls).toEqual([]);
    });

    it('stops at a slice boundary, reporting the first block it did not do', async () => {
      const abort = new AbortController();
      const seen: [number, number][] = [];
      const stopper: BlockProcessor = {
        name: 'stopper',
        onBlockRange: (from, to) => {
          seen.push([from, to]);
          abort.abort();
          return ok();
        },
        onReorg: () => ok(),
      };
      const { runner } = harness({ processors: [stopper] });

      const result = await runner.run(request(), abort.signal);

      expect(result).toEqual({ kind: 'aborted', resumeFrom: 150 });
      expect(seen).toEqual([[100, 149]]);
    });
  });

  describe('a dry run', () => {
    it('reports the plan without dispatching', async () => {
      const { runner, processors } = harness();

      const result = await runner.run(request({ dryRun: true }), NEVER_ABORTS);

      expect(result).toEqual({ kind: 'completed', ranges: 3, blocks: 150 });
      expect(processors[0]!.calls).toEqual([]);
    });

    it('still refuses a range above the safe head', async () => {
      const { runner } = harness({ safeHead: 900 });

      expect(
        await runner.run(request({ from: 800, to: 950, dryRun: true }), NEVER_ABORTS),
      ).toMatchObject({ kind: 'failed' });
    });

    it('still refuses an unknown processor name', async () => {
      const { runner } = harness();

      expect(
        await runner.run(request({ processors: ['nope'], dryRun: true }), NEVER_ABORTS),
      ).toMatchObject({ kind: 'failed' });
    });
  });
});
