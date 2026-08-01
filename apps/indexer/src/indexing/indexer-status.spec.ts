import { describe, expect, it } from 'vitest';

import { IndexerStatus } from './indexer-status';

describe('IndexerStatus', () => {
  it('starts with nothing known about the chain', () => {
    expect(new IndexerStatus().snapshot).toMatchObject({
      state: 'starting',
      reason: null,
      lastBlock: null,
      head: null,
      consecutiveFailures: 0,
    });
  });

  it('records the cursor position as it advances', () => {
    const status = new IndexerStatus();

    status.progressed(500);

    expect(status.snapshot).toMatchObject({ state: 'running', lastBlock: 500 });
  });

  it('counts consecutive failures and clears them on progress', () => {
    const status = new IndexerStatus();

    status.retried('rpc timeout');
    status.retried('rpc timeout');
    expect(status.consecutiveFailures).toBe(2);

    status.progressed(500);
    expect(status.consecutiveFailures).toBe(0);
    expect(status.snapshot.reason).toBeNull();
  });

  it('treats being caught up as progress', () => {
    const status = new IndexerStatus();
    status.retried('rpc timeout');
    const before = status.snapshot.lastProgressAt;

    status.idled();

    // A quiet chain is the healthy steady state; the stall alarm keys off this,
    // so idling must not look like being stuck.
    expect(status.snapshot.state).toBe('running');
    expect(status.snapshot.lastProgressAt).toBeGreaterThanOrEqual(before);
    expect(status.consecutiveFailures).toBe(0);
  });

  it('holds the failure reason, and stays failed', () => {
    const status = new IndexerStatus();

    status.failed('chain id mismatch');

    expect(status.isFailed).toBe(true);
    expect(status.failureReason).toBe('chain id mismatch');
  });

  it('reports a retry reason without becoming terminal', () => {
    const status = new IndexerStatus();

    status.retried('rpc timeout');

    expect(status.isFailed).toBe(false);
    expect(status.snapshot).toMatchObject({ state: 'retrying', reason: 'rpc timeout' });
  });

  describe('observeHead', () => {
    it('returns the observed head while it climbs', () => {
      const status = new IndexerStatus();

      expect(status.observeHead(1_000)).toBe(1_000);
      expect(status.observeHead(1_010)).toBe(1_010);
    });

    it('clamps a head that goes backwards', () => {
      const status = new IndexerStatus();
      status.observeHead(1_000);

      // A failover onto a lagging provider. Taken at face value this inverts a
      // range, or reads as a reorg that never happened.
      expect(status.observeHead(940)).toBe(1_000);
      expect(status.snapshot.head).toBe(1_000);
    });

    it('keeps tracking the head while the loop fails to progress', () => {
      const status = new IndexerStatus();
      status.progressed(500);
      status.observeHead(1_000);

      status.retried('rpc timeout');
      status.observeHead(1_050);

      // Why the head is observed separately from a transition rather than
      // folded into progressed(): lag is head - lastBlock, so a stuck indexer
      // whose head froze at its last success would report a lag that stops
      // growing exactly when it starts mattering.
      expect(status.snapshot).toMatchObject({ state: 'retrying', lastBlock: 500, head: 1_050 });
    });
  });
});
