import { describe, expect, it } from 'vitest';

import { PendingTokens } from './pending-tokens';

const USDC = '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48';
const WETH = '0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2';

describe('PendingTokens', () => {
  it('is empty until ingestion puts something in it', () => {
    // Which is the state of every dispatch after genesis, and the reason the
    // consumer can answer "anything to do?" without a query.
    expect(new PendingTokens().size).toBe(0);
  });

  it('hands back what was added, then forgets it', () => {
    const pending = new PendingTokens();
    pending.add([USDC, WETH]);

    expect([...pending.drain()].toSorted()).toEqual([USDC, WETH].toSorted());
    expect(pending.size).toBe(0);
    expect(pending.drain()).toEqual([]);
  });

  it('keeps one entry for a token added twice', () => {
    // A range re-dispatched after a later processor retries replays the same
    // events, and two asset ids sharing an underlying is not ruled out.
    const pending = new PendingTokens();
    pending.add([USDC]);
    pending.add([USDC]);

    expect(pending.size).toBe(1);
  });

  it('lower-cases, because both sides of the diff do', () => {
    const pending = new PendingTokens();
    pending.add(['0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48']);

    // The store writes lower-case and the fold stores lower-case; a
    // checksummed entry here would read as a token nobody has enriched.
    expect(pending.drain()).toEqual([USDC]);
  });

  it('accumulates across several ranges before anything drains it', () => {
    // The consumer runs at most one job at a time, so listings can arrive
    // while it is busy. None may be lost for that.
    const pending = new PendingTokens();
    pending.add([USDC]);
    pending.add([WETH]);

    expect([...pending.drain()].toSorted()).toEqual([USDC, WETH].toSorted());
  });
});
