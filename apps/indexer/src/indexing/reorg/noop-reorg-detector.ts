import { Inject, Injectable } from '@nestjs/common';

import { INDEXING_OPTIONS, type IndexingOptions } from '../indexing.options';
import type { ReorgDetector, ReorgVerdict } from './reorg-detector';

/**
 * Placeholder detector: answers the finality question, detects nothing.
 *
 * `safeHead` is real — it is what keeps the loop dispatching wide ranges while
 * catching up and single blocks near the tip. Everything else is inert:
 * `inspect` always reports `continuous`, so **no reorg is ever detected in a
 * running service**, and `bootstrap` retains nothing, so a fork that happens
 * across a restart passes unnoticed too.
 *
 * The name is deliberate. The loop's reorg and bootstrap paths are fully
 * exercised by tests through a scripted detector; the hash-chain implementation
 * that makes them live is a separate change.
 */
@Injectable()
export class NoopReorgDetector implements ReorgDetector {
  constructor(@Inject(INDEXING_OPTIONS) private readonly options: IndexingOptions) {}

  bootstrap(): Promise<ReorgVerdict> {
    return Promise.resolve({ type: 'continuous' });
  }

  /**
   * Clamped at `-1` so a chain shorter than the finality depth — a freshly
   * started Anvil, say — yields no negative range and simply treats every block
   * as unsettled.
   */
  safeHead(observedHead: number): number {
    return Math.max(-1, observedHead - this.options.finalityDepth);
  }

  inspect(): ReorgVerdict {
    return { type: 'continuous' };
  }

  commit(): void {
    // Nothing retained, so nothing to record.
  }

  rewindTo(): void {
    // Nothing retained, so nothing to discard.
  }
}
