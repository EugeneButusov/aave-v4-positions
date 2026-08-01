import type { HealthIndicator } from '@packages/ops';
import { Inject, Injectable } from '@nestjs/common';

import { IndexerStatus } from './indexer-status';
import { INDEXING_OPTIONS, type IndexingOptions } from '../indexing.options';

/**
 * Surfaces the indexing loop on `/health/ready`.
 *
 * Two conditions only: the loop has stopped, or it has stopped making progress.
 * Anything richer — how far behind the tip we are, reconciliation drift —
 * belongs to the reconciliation work, not here.
 *
 * Worth being clear about what this buys operationally. The indexer serves
 * probes and nothing else, so failing readiness drains no traffic, and Docker's
 * `restart: unless-stopped` does not react to an unhealthy healthcheck. This is
 * an **alertable signal with no automatic recovery** — a human or a
 * Kubernetes-level policy has to act on it.
 */
@Injectable()
export class IndexerHealthIndicator implements HealthIndicator {
  readonly name = 'indexer';

  constructor(
    private readonly status: IndexerStatus,
    @Inject(INDEXING_OPTIONS) private readonly options: IndexingOptions,
  ) {}

  check(): void {
    const { state, reason, lastBlock, head, lastProgressAt } = this.status.snapshot;

    if (state === 'failed') {
      throw new Error(`indexing stopped: ${reason ?? 'unknown reason'}`);
    }

    // Before the first iteration there is nothing to judge, and reporting down
    // here would stop the pod ever becoming ready.
    if (state === 'starting') return;

    const stalledForMs = Date.now() - lastProgressAt;
    if (stalledForMs > this.options.stallThresholdMs) {
      throw new Error(
        `no progress for ${Math.round(stalledForMs / 1_000)}s at block ${lastBlock ?? 'none'} (head ${head ?? 'unknown'})`,
      );
    }
  }
}
