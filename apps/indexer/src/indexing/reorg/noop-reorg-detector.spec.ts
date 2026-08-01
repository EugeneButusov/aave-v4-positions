import { describe, expect, it } from 'vitest';

import type { IndexingOptions } from '../indexing.options';
import { NoopReorgDetector } from './noop-reorg-detector';

function detector(finalityDepth: number): NoopReorgDetector {
  return new NoopReorgDetector({ finalityDepth } as IndexingOptions);
}

describe('NoopReorgDetector', () => {
  it('places the safe head one finality depth below the observed head', () => {
    expect(detector(128).safeHead(1_000)).toBe(872);
  });

  it('clamps the safe head rather than going negative on a short chain', () => {
    // A freshly started Anvil is a handful of blocks deep. Without the clamp
    // the loop would compare against a negative bound and dispatch an inverted
    // range.
    expect(detector(128).safeHead(5)).toBe(-1);
  });

  it('treats every block as unsettled when nothing is final', () => {
    expect(detector(128).safeHead(128)).toBe(0);
  });

  it('never reports a reorg', () => {
    expect(detector(128).inspect()).toEqual({ type: 'continuous' });
  });

  it('accepts any resume point as canonical', async () => {
    await expect(detector(128).bootstrap()).resolves.toEqual({ type: 'continuous' });
  });
});
