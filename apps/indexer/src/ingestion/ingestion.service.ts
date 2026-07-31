import {
  Inject,
  Injectable,
  Logger,
  Optional,
  type OnApplicationBootstrap,
  type OnApplicationShutdown,
} from '@nestjs/common';

import { EVENT_SOURCES, type EventSource } from './event-source';

/**
 * Owns the lifetime of every registered {@link EventSource}: starts them on
 * bootstrap and aborts them on shutdown, so a SIGTERM stops ingestion at a
 * known point rather than mid-batch.
 *
 * No sources are registered yet — the ingestion pipeline itself is the next
 * increment. This class exists so that adding one is a provider binding rather
 * than a lifecycle rewrite.
 */
@Injectable()
export class IngestionService implements OnApplicationBootstrap, OnApplicationShutdown {
  private readonly logger = new Logger(IngestionService.name);
  private readonly abort = new AbortController();
  private running: Promise<void>[] = [];

  constructor(
    @Optional()
    @Inject(EVENT_SOURCES)
    private readonly sources: EventSource[] = [],
  ) {}

  onApplicationBootstrap(): void {
    if (this.sources.length === 0) {
      this.logger.warn('no event sources registered; the indexer is idle');
      return;
    }

    this.running = this.sources.map((source) => this.supervise(source));
    this.logger.log(`started ${this.sources.length} event source(s)`);
  }

  async onApplicationShutdown(): Promise<void> {
    if (this.running.length === 0) return;

    this.abort.abort();
    await Promise.allSettled(this.running);
    this.logger.log('all event sources stopped');
  }

  get sourceNames(): string[] {
    return this.sources.map((s) => s.name);
  }

  /**
   * A source that throws must not take the process down with it — that would
   * turn one bad feed into a crash loop for every other feed in the pod. The
   * failure is logged and left for readiness to surface.
   */
  private async supervise(source: EventSource): Promise<void> {
    try {
      await source.start(this.abort.signal);
    } catch (error) {
      if (this.abort.signal.aborted) return;
      this.logger.error(
        `event source "${source.name}" stopped unexpectedly`,
        error instanceof Error ? error.stack : String(error),
      );
    }
  }
}
