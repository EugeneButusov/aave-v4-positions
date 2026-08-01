import {
  LogRangeTooLargeError,
  failed,
  ok,
  retry,
  type Address,
  type BlockProcessor,
  type LogReader,
  type ProcessorOutcome,
} from '@packages/indexing';
import { Logger } from '@nestjs/common';

import { SPOKE_POSITION_TOPICS } from './aave/spoke-events';
import { SpokeEventDecoder, UndecodableLogError } from './decode/decoder';
import type { EventStore } from './store/event-store';

export interface AaveEventProcessorOptions {
  readonly chainId: number;
  /** Which Spoke this processor follows. A second Spoke is a second instance. */
  readonly spoke: Address;
}

/**
 * Reads one Spoke's position events over the dispatched range and writes them.
 *
 * The Spoke is configuration rather than a constant, so running a second Spoke
 * is another registration with a different address — see
 * {@link aaveEventProcessor}. Nothing about the pipeline is per-Spoke beyond
 * this option.
 */
export class AaveEventProcessor implements BlockProcessor {
  readonly name: string;

  private readonly logger: Logger;
  private readonly decoder: SpokeEventDecoder;

  constructor(
    private readonly options: AaveEventProcessorOptions,
    private readonly logs: LogReader,
    private readonly store: EventStore,
  ) {
    // Names the Spoke, so a retry reason says which one stalled once there is
    // more than one.
    this.name = `aave-events(${options.spoke.slice(0, 10)})`;
    this.logger = new Logger(this.name);
    this.decoder = new SpokeEventDecoder(options.chainId, options.spoke);
  }

  /**
   * Revert then append, never one without the other: dispatch is at-least-once
   * and the engine collapses no repeated insert on its own, so an append that
   * skipped the revert would leave two live copies of the range.
   */
  async onBlockRange(from: number, to: number, signal: AbortSignal): Promise<ProcessorOutcome> {
    let events;
    try {
      const raw = await this.logs.getLogs({
        addresses: [this.options.spoke],
        topic0: SPOKE_POSITION_TOPICS,
        fromBlock: from,
        toBlock: to,
      });
      events = this.decoder.decode(raw);
    } catch (error) {
      if (error instanceof LogRangeTooLargeError) {
        return retry(error.message, { narrowRange: true });
      }
      if (error instanceof UndecodableLogError) {
        // Terminal on purpose. We asked the provider for exactly these eight
        // topics, so a log that will not decode means the ABI or the filter is
        // wrong, and no number of retries fixes either.
        return failed(error.message);
      }
      throw error;
    }

    // Don't open a fresh write on the way out. The cursor has not advanced, so
    // the range is simply re-dispatched on the next start.
    if (signal.aborted) return retry('shutting down before write');

    await this.store.revert(this.options.chainId, from, to);
    await this.store.append(events);

    if (events.length > 0) {
      this.logger.log(`blocks ${from}..${to}: stored ${events.length} event(s)`);
    }
    return ok();
  }

  /**
   * Discards everything derived from `[from, to]`.
   *
   * The first half of what the range path does, with nothing to put back — so a
   * reorg and a retry retract by the same call rather than two mechanisms.
   */
  async onReorg(from: number, to: number): Promise<ProcessorOutcome> {
    this.logger.warn(`reorg: retracting blocks ${from}..${to}`);
    await this.store.revert(this.options.chainId, from, to);
    return ok();
  }
}
