import type { Address, RawLog } from '@packages/indexing';
import { decodeEventLog, type Abi } from 'viem';

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
 * Turns one contract's logs into rows.
 *
 * **Scoped to one emitting address**, which §4.5 makes a requirement rather
 * than a nicety: `ReportDeficit` exists on both the Spoke and the Hub with
 * different signatures and different topic0s, and `Add`, `Remove`, `Draw` and
 * `Withdraw` are generic enough to collide across contracts. Decoding a merged
 * stream by topic0 alone is how a Hub event silently becomes a wrong Spoke row.
 *
 * The address check is what makes that structural rather than a convention —
 * a subclass cannot opt out of it, and a log from the wrong contract is
 * rejected before its ABI is ever consulted.
 *
 * Nothing is skipped. The caller asked the provider for exactly these topics,
 * so a log that arrives and will not decode is a contradiction — a changed ABI,
 * or a filter that did not do what it claimed — and it throws rather than
 * quietly shrinking the result.
 *
 * One subclass per contract, each beside the ABI it decodes against — see
 * `spoke-event-decoder.ts` and `hub-event-decoder.ts`. All either supplies is an
 * ABI, an event filter and a name for the rejection message; everything that
 * decides what a row looks like is here, so the two ledgers cannot drift apart
 * in shape.
 */
export abstract class ContractLogDecoder {
  private readonly contract: Address;

  constructor(
    private readonly chainId: number,
    contract: Address,
  ) {
    // Normalised here rather than trusted: a checksummed address from a caller
    // would match no log and the decoder would reject every one of them.
    this.contract = contract.toLowerCase();
  }

  /** The ABI every log is decoded against — the emitting contract's, and only its. */
  protected abstract readonly abi: Abi;

  /** Which decoded events this decoder was asked for. */
  protected abstract wanted(eventName: string): boolean;

  /** What this contract is called, so a rejection names it. */
  protected abstract readonly role: string;

  decode(logs: readonly RawLog[]): DecodedEvent[] {
    return logs.map((log) => this.decodeOne(log));
  }

  private decodeOne(log: RawLog): DecodedEvent {
    if (log.address.toLowerCase() !== this.contract) {
      throw new UndecodableLogError(
        log.blockNumber,
        log.logIndex,
        `emitted by ${log.address}, not the configured ${this.role} ${this.contract}`,
      );
    }

    const [signature, ...rest] = log.topics;
    if (signature === undefined) {
      throw new UndecodableLogError(log.blockNumber, log.logIndex, 'anonymous event, no topic0');
    }

    // Typed as optional because the ABI is widened to `Abi` here rather than
    // held as a const — viem can no longer prove a name comes back.
    let eventName: string | undefined;
    let rawArgs: unknown;
    try {
      ({ eventName, args: rawArgs } = decodeEventLog({
        abi: this.abi,
        data: log.data,
        topics: [signature, ...rest],
      }));
    } catch (error) {
      const reason = error instanceof Error ? error.message : String(error);
      throw new UndecodableLogError(log.blockNumber, log.logIndex, reason);
    }

    if (eventName === undefined || !this.wanted(eventName)) {
      throw new UndecodableLogError(
        log.blockNumber,
        log.logIndex,
        `${eventName ?? '<unnamed>'} is not one of the ${this.role} events that were requested`,
      );
    }

    // topic0 is the signature hash, which `event_name` already says. What is
    // left is the indexed parameters, in ABI order — so their meaning is
    // per-event, and per-contract, and is read through a per-event view.
    const [topic1, topic2, topic3] = rest;

    return {
      chainId: this.chainId,
      address: this.contract,
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
