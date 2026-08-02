import {
  LogRangeTooLargeError,
  failed,
  ok,
  retry,
  type Address,
  type BlockProcessor,
  type Hash,
  type LogReader,
  type ProcessorOutcome,
} from '@packages/indexing';
import { Logger } from '@nestjs/common';

import { HUB_STATE_TOPICS, listedTokens } from './aave/hub-events';
import { SPOKE_POSITION_TOPICS } from './aave/spoke-events';
import type { DecodedEvent } from './decode/decoded-event';
import { UndecodableLogError, type ContractLogDecoder } from './decode/decoder';
import { HubEventDecoder } from './decode/hub-event-decoder';
import { SpokeEventDecoder } from './decode/spoke-event-decoder';
import type { EventStore } from './store/event-store';

/**
 * One contract's stream: which address, which topics, and how to read them.
 *
 * The three travel together because they have to agree — asking for topics the
 * decoder will not accept turns every log into a terminal failure, and the
 * built sources below are what keep them in step.
 */
export interface EventSource {
  readonly chainId: number;
  /** Names the contract kind in logs and retry reasons: `spoke`, `hub`. */
  readonly role: string;
  readonly contract: Address;
  readonly topics: readonly Hash[];
  readonly decoder: ContractLogDecoder;
  /**
   * Told what was just written, after it is written.
   *
   * **Write-only, and never awaited.** A listener exists so a downstream
   * concern can react to an event it would otherwise have to go and look for —
   * re-reading the ledger every range to rediscover what this processor had in
   * hand and threw away. Ingestion must not slow down or fail for one, so its
   * return value is ignored and it is called after the append rather than
   * before: nothing here is a second write path.
   */
  readonly onAppended?: (events: readonly DecodedEvent[]) => void;
}

export function spokeEventSource(chainId: number, spoke: Address): EventSource {
  return {
    chainId,
    role: 'spoke',
    contract: spoke,
    topics: SPOKE_POSITION_TOPICS,
    decoder: new SpokeEventDecoder(chainId, spoke),
  };
}

/**
 * @param onListed told which ERC-20s a range listed, as it is ingested.
 *
 * The Hub is where a token first appears, and `AddAsset` carries the address —
 * so this is the one place the set of listed tokens is known without asking a
 * database for it. Enrichment subscribes rather than polling.
 */
export function hubEventSource(
  chainId: number,
  hub: Address,
  onListed?: (tokens: readonly Address[]) => void,
): EventSource {
  return {
    chainId,
    role: 'hub',
    contract: hub,
    topics: HUB_STATE_TOPICS,
    decoder: new HubEventDecoder(chainId, hub),
    ...(onListed && {
      onAppended: (events): void => {
        const tokens = listedTokens(events);
        if (tokens.length > 0) onListed(tokens);
      },
    }),
  };
}

/**
 * Reads one contract's events over the dispatched range and writes them.
 *
 * The contract is configuration rather than a constant, so a second Spoke — or
 * the Hub — is another registration with a different {@link EventSource} and
 * its own store. Nothing about the pipeline is per-contract beyond that.
 */
export class AaveEventProcessor implements BlockProcessor {
  readonly name: string;

  private readonly logger: Logger;

  constructor(
    private readonly source: EventSource,
    private readonly logs: LogReader,
    private readonly store: EventStore,
  ) {
    // Names the contract, so a retry reason says which stream stalled once
    // there is more than one.
    this.name = `aave-${source.role}(${source.contract.slice(0, 10)})`;
    this.logger = new Logger(this.name);
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
        addresses: [this.source.contract],
        topic0: this.source.topics,
        fromBlock: from,
        toBlock: to,
      });
      events = this.source.decoder.decode(raw);
    } catch (error) {
      if (error instanceof LogRangeTooLargeError) {
        return retry(error.message, { narrowRange: true });
      }
      if (error instanceof UndecodableLogError) {
        // Terminal on purpose. We asked the provider for exactly these topics,
        // so a log that will not decode means the ABI or the filter is wrong,
        // and no number of retries fixes either.
        return failed(error.message);
      }
      throw error;
    }

    // Don't open a fresh write on the way out. The cursor has not advanced, so
    // the range is simply re-dispatched on the next start.
    if (signal.aborted) return retry('shutting down before write');

    await this.store.revert(this.source.chainId, from, to);
    await this.store.append(events);

    if (events.length > 0) {
      this.logger.log(`blocks ${from}..${to}: stored ${events.length} event(s)`);
    }

    // After the write, and not awaited. A listener is a downstream concern
    // reacting to what landed; it must not be able to fail or delay ingestion.
    this.source.onAppended?.(events);

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
    await this.store.revert(this.source.chainId, from, to);
    return ok();
  }
}
