import type { Address, RawLog } from '@packages/indexing';
import { decodeEventLog } from 'viem';

import { SPOKE_ABI, isPositionEvent } from '../aave/spoke-events';
import type { DecodedEvent } from './decoded-event';

/** A log the decoder refuses, named so the failure says which log and why. */
export class UndecodableLogError extends Error {
  constructor(
    readonly blockNumber: number,
    readonly logIndex: number,
    reason: string,
  ) {
    super(`cannot decode log ${blockNumber}:${logIndex}: ${reason}`);
    this.name = 'UndecodableLogError';
  }
}

/** Widens a decoded arg bag to something indexable, without asserting a shape. */
function toRecord(value: unknown): Record<string, unknown> {
  if (typeof value !== 'object' || value === null) return {};
  const entries: [string, unknown][] = Object.entries(value);
  return Object.fromEntries(entries);
}

/** `JSON.stringify` throws on a bigint, and float64 would corrupt it anyway (§7.5). */
function jsonSafe(value: unknown): unknown {
  if (typeof value === 'bigint') return value.toString();
  if (Array.isArray(value)) return (value as unknown[]).map(jsonSafe);
  if (typeof value === 'object' && value !== null) {
    return Object.fromEntries(Object.entries(toRecord(value)).map(([k, v]) => [k, jsonSafe(v)]));
  }
  return value;
}

/**
 * Turns Spoke logs into rows.
 *
 * **Scoped to one emitting address**, which §4.5 makes a requirement rather
 * than a nicety: `ReportDeficit` exists on both the Spoke and the Hub with
 * different signatures and different topic0s, and `Add`, `Remove`, `Draw` and
 * `Withdraw` are generic enough to collide across contracts. Decoding a merged
 * stream by topic0 alone is how a Hub event silently becomes a wrong Spoke row.
 * Only the Spoke is in scope today; the decoder is keyed by address anyway, so
 * adding the Hub cannot introduce that bug later.
 *
 * Nothing is skipped. The caller asked the provider for exactly these eight
 * topics, so a log that arrives and will not decode is a contradiction — a
 * changed ABI, or a filter that did not do what it claimed — and it throws
 * rather than quietly shrinking the result.
 */
export class SpokeEventDecoder {
  private readonly spoke: Address;

  constructor(
    private readonly chainId: number,
    spoke: Address,
  ) {
    // Normalised here rather than trusted: a checksummed address from a caller
    // would match no log and the decoder would reject every one of them.
    this.spoke = spoke.toLowerCase();
  }

  decode(logs: readonly RawLog[]): DecodedEvent[] {
    return logs.map((log) => this.decodeOne(log));
  }

  private decodeOne(log: RawLog): DecodedEvent {
    if (log.address.toLowerCase() !== this.spoke) {
      throw new UndecodableLogError(
        log.blockNumber,
        log.logIndex,
        `emitted by ${log.address}, not the configured spoke ${this.spoke}`,
      );
    }

    const [signature, ...rest] = log.topics;
    if (signature === undefined) {
      throw new UndecodableLogError(log.blockNumber, log.logIndex, 'anonymous event, no topic0');
    }

    let eventName: string;
    let rawArgs: unknown;
    try {
      ({ eventName, args: rawArgs } = decodeEventLog({
        abi: SPOKE_ABI,
        data: log.data,
        topics: [signature, ...rest],
      }));
    } catch (error) {
      const reason = error instanceof Error ? error.message : String(error);
      throw new UndecodableLogError(log.blockNumber, log.logIndex, reason);
    }

    if (!isPositionEvent(eventName)) {
      throw new UndecodableLogError(
        log.blockNumber,
        log.logIndex,
        `${eventName} is not one of the position events that were requested`,
      );
    }

    // topic0 is the signature hash, which `event_name` already says. What is
    // left is the indexed parameters, in ABI order — so topic1 is the reserve
    // id on every one of the eight, while topic2 and topic3 mean different
    // things per event and are read through a per-event view.
    const [topic1, topic2, topic3] = rest;

    return {
      chainId: this.chainId,
      address: this.spoke,
      blockNumber: log.blockNumber,
      blockHash: log.blockHash,
      blockTimestamp: log.blockTimestamp,
      txHash: log.transactionHash,
      txIndex: log.transactionIndex,
      logIndex: log.logIndex,
      eventName,
      topic1: topic1 ?? null,
      topic2: topic2 ?? null,
      topic3: topic3 ?? null,
      body: toRecord(jsonSafe(toRecord(rawArgs))),
      data: log.data,
    };
  }
}
