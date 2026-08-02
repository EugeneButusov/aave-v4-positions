import type { HealthIndicator } from '@packages/ops';
import { Inject, Injectable, Logger } from '@nestjs/common';

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

  private readonly logger = new Logger(IndexerHealthIndicator.name);

  /**
   * Whether the last check found the loop stalled.
   *
   * Held so the stall is logged **once, on each transition** rather than on
   * every probe. The healthcheck runs every ten seconds, so a line per check
   * would put six identical entries a minute into the stream for as long as the
   * outage lasts — burying the one line that says when it started.
   */
  private stalled = false;

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
    const seconds = Math.round(stalledForMs / 1_000);

    if (stalledForMs > this.options.stallThresholdMs) {
      // Logged as well as thrown. Throwing reaches the probe and nothing else,
      // so before this a stalled indexer produced **no log line at all** — the
      // condition was visible only to whoever thought to curl `/health/ready`.
      if (!this.stalled) {
        this.stalled = true;
        this.logger.error(
          { stalledForSeconds: seconds, lastBlock, head },
          'indexer has stopped making progress',
        );
      }
      throw new Error(
        `no progress for ${seconds}s at block ${lastBlock ?? 'none'} (head ${head ?? 'unknown'})`,
      );
    }

    if (this.stalled) {
      this.stalled = false;
      this.logger.log({ lastBlock, head }, 'indexer is making progress again');
    }
  }
}
