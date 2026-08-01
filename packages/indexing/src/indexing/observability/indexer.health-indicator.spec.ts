import { describe, expect, it } from 'vitest';

import type { IndexerSnapshot, IndexerStatus } from './indexer-status';
import { IndexerHealthIndicator } from './indexer.health-indicator';
import type { IndexingOptions } from '../indexing.options';

const STALL_THRESHOLD_MS = 60_000;

function indicator(snapshot: Partial<IndexerSnapshot>): IndexerHealthIndicator {
  const full: IndexerSnapshot = {
    state: 'running',
    reason: null,
    lastBlock: 500,
    head: 1_000,
    lastProgressAt: Date.now(),
    consecutiveFailures: 0,
    ...snapshot,
  };

  return new IndexerHealthIndicator(
    { snapshot: full } as IndexerStatus,
    {
      stallThresholdMs: STALL_THRESHOLD_MS,
    } as IndexingOptions,
  );
}

describe('IndexerHealthIndicator', () => {
  it('reports up before the loop has run', () => {
    // A pod that never becomes ready is worse than one that is briefly
    // optimistic: readiness gates startup, and there is nothing to judge yet.
    expect(() => indicator({ state: 'starting', lastProgressAt: 0 }).check()).not.toThrow();
  });

  it('reports up while making progress', () => {
    expect(() => indicator({ state: 'running' }).check()).not.toThrow();
  });

  it('reports up while retrying, as long as it is still trying', () => {
    expect(() => indicator({ state: 'retrying' }).check()).not.toThrow();
  });

  it('reports down once the loop has stopped, naming why', () => {
    expect(() => indicator({ state: 'failed', reason: 'chain id mismatch' }).check()).toThrow(
      /indexing stopped: chain id mismatch/,
    );
  });

  it('reports down when progress has stalled, naming the position', () => {
    const check = (): void =>
      indicator({
        state: 'retrying',
        lastBlock: 24_720_000,
        head: 24_800_000,
        lastProgressAt: Date.now() - STALL_THRESHOLD_MS - 5_000,
      }).check();

    // The readiness payload carries only the thrown message, so the detail has
    // to be in it rather than in a structured field.
    expect(check).toThrow(/no progress for 6[45]s at block 24720000 \(head 24800000\)/);
  });

  it('tolerates a gap just under the threshold', () => {
    expect(() =>
      indicator({ lastProgressAt: Date.now() - STALL_THRESHOLD_MS + 5_000 }).check(),
    ).not.toThrow();
  });
});
