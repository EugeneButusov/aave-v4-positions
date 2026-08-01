import { Injectable, Logger } from '@nestjs/common';

import { ok, type BlockProcessor, type ProcessorOutcome } from './block-processor';

/**
 * Placeholder processor: reports what the loop dispatched and always succeeds.
 *
 * It exists so the framework has a registered processor end to end — proving
 * the multi-provider token, the ordering guarantee and the outcome gating are
 * wired — and so `docker compose up` shows the loop walking the chain. It
 * decodes nothing and stores nothing.
 */
@Injectable()
export class LoggingBlockProcessor implements BlockProcessor {
  readonly name = 'logging';

  private readonly logger = new Logger(LoggingBlockProcessor.name);

  onBlockRange(from: number, to: number): ProcessorOutcome {
    this.logger.log(from === to ? `block ${from}` : `blocks ${from}..${to} (${to - from + 1})`);
    return ok();
  }

  onReorg(from: number, to: number): ProcessorOutcome {
    this.logger.warn(`reorg: discarding blocks ${from}..${to}`);
    return ok();
  }
}
