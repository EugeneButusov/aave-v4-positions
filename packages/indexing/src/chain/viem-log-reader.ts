import { Inject, Injectable, Logger } from '@nestjs/common';
import { BaseError, hexToNumber, numberToHex, type RpcLog } from 'viem';

import { CHAIN_CLIENT_OPTIONS, type ChainClientOptions, type Hex } from './chain-client';
import { LogRangeTooLargeError, type LogFilter, type LogReader, type RawLog } from './log-reader';
import { ViemChainClient } from './viem-chain-client';

/**
 * viem's `RpcLog` is the wire shape with every quantity still hex, and it
 * already carries the optional `blockTimestamp` — so it is used directly rather
 * than re-declared here.
 *
 * Its block fields are nullable because the same type covers *pending* logs.
 * None can reach us: `fromBlock` and `toBlock` are concrete heights, and a
 * pending log belongs to no block. {@link minedLog} makes that assumption
 * explicit and loud instead of asserting it away.
 */
interface MinedLog {
  readonly blockNumber: Hex;
  readonly blockHash: Hex;
  readonly transactionHash: Hex;
  readonly transactionIndex: Hex;
  readonly logIndex: Hex;
}

function minedLog(log: RpcLog): MinedLog {
  const { blockNumber, blockHash, transactionHash, transactionIndex, logIndex } = log;

  if (
    blockNumber === null ||
    blockHash === null ||
    transactionHash === null ||
    transactionIndex === null ||
    logIndex === null
  ) {
    throw new Error('eth_getLogs returned a pending log for a bounded block range');
  }

  return { blockNumber, blockHash, transactionHash, transactionIndex, logIndex };
}

/**
 * Does this provider message mean "ask for fewer blocks"?
 *
 * Matched on the message rather than the error class or JSON-RPC code, because
 * neither distinguishes the cases. Measured against four public endpoints on
 * 2026-08-01, `1rpc.io` and `ethereum-rpc.publicnode.com` both answer with
 * viem's `InvalidParamsRpcError` and code `-32602` — but one means the range is
 * too wide and the other means the plan does not cover historical data. Only the
 * text tells them apart, and narrowing the second forever would never help.
 *
 * Both halves must hit. The verified positives:
 *
 * - `ranges over 10000 blocks are not supported on free plan` — drpc
 * - `eth_getLogs is limited to 0 - 50 blocks range` — 1rpc
 * - `Block range too large: maximum allowed is 50 blocks…` — nodies
 *
 * and the verified negative, which must *not* match:
 *
 * - `Archive requests require a personal token…` — publicnode
 *
 * A provider whose phrasing escapes this falls through to an ordinary retry: the
 * loop keeps trying at the same width, which makes no progress but corrupts
 * nothing. Widen the pattern when a new one turns up, with its message recorded
 * in the spec.
 */
const RANGE_SUBJECT = /\b(ranges?|results?)\b/i;
const RANGE_LIMIT =
  /\b(too large|too many|exceed(s|ed)?|not supported|limited to|maximum|max|limit)\b/i;

function isRangeRejection(detail: string): boolean {
  return RANGE_SUBJECT.test(detail) && RANGE_LIMIT.test(detail);
}

/** viem puts the provider's own message on `details`; everything else is its own framing. */
function providerDetail(error: unknown): string {
  return error instanceof BaseError ? (error.details ?? error.shortMessage) : '';
}

/**
 * Adapts viem to the {@link LogReader} port.
 *
 * Goes through `client.request` rather than viem's typed `getLogs`. That action
 * derives its topic filter from an ABI and **silently ignores a `topics`
 * argument** — verified: the same 124 logs come back with and without one. A
 * filter that quietly does nothing would mean fetching every event the contract
 * emits while believing we had narrowed to eight, so the raw call is the honest
 * one here. It also returns the log unformatted, which is exactly what this port
 * wants: decoding belongs to whoever owns the ABI.
 *
 * Extends {@link ViemChainClient} rather than restating how a transport is
 * built, and fills a missing `blockTimestamp` through the inherited
 * {@link ViemChainClient.getBlockHeader}. It is bound as its own provider, so
 * the loop and a processor hold separate connections over the same ordered
 * provider list.
 */
@Injectable()
export class ViemLogReader extends ViemChainClient implements LogReader {
  private readonly logger = new Logger(ViemLogReader.name);

  // Declared rather than inherited so the injection metadata sits on this
  // class; Nest does not walk up to a base constructor's parameter decorators.
  constructor(@Inject(CHAIN_CLIENT_OPTIONS) options: ChainClientOptions) {
    super(options);
  }

  async getLogs(filter: LogFilter): Promise<RawLog[]> {
    const logs = await this.request(filter);
    return this.withTimestamps(logs);
  }

  private async request(filter: LogFilter): Promise<RpcLog[]> {
    // Two shape differences between the port and viem, both settled here so
    // neither leaks outward: viem wants mutable arrays rather than the port's
    // readonly ones, and it types an address as a `0x${string}` template where
    // `Address` is a plain string, because addresses get lower-cased on the way
    // in and `toLowerCase()` returns `string`. This is the adapter boundary, so
    // it is the right place for the one cast that reconciles them.
    // oxlint-disable-next-line no-unsafe-type-assertion
    const address = [...filter.addresses] as `0x${string}`[];

    const params = {
      address,
      fromBlock: numberToHex(filter.fromBlock),
      toBlock: numberToHex(filter.toBlock),
      ...(filter.topic0.length > 0 && { topics: [[...filter.topic0]] }),
    };

    try {
      return await this.client.request({ method: 'eth_getLogs', params: [params] });
    } catch (error) {
      const detail = providerDetail(error);
      if (isRangeRejection(detail)) {
        throw new LogRangeTooLargeError(filter.fromBlock, filter.toBlock, detail);
      }
      throw error;
    }
  }

  /**
   * Fills `blockTimestamp` for providers that omit it, by reading the headers of
   * the distinct blocks that actually carry logs — at most one request per block,
   * never one per log.
   *
   * The common path costs nothing: drpc and mevblocker both return the field, so
   * this returns immediately without a single extra call.
   */
  private async withTimestamps(logs: RpcLog[]): Promise<RawLog[]> {
    const mined = logs.map((log) => [log, minedLog(log)] as const);

    const missing = [
      ...new Set(
        mined.filter(([log]) => log.blockTimestamp === undefined).map(([, m]) => m.blockNumber),
      ),
    ];

    const fetched = new Map<Hex, number>();
    if (missing.length > 0) {
      this.logger.warn(
        { headers: missing.length },
        'provider omitted blockTimestamp; reading block headers to fill it',
      );
      const headers = await Promise.all(
        missing.map(async (blockNumber) => {
          const header = await this.getBlockHeader(hexToNumber(blockNumber));
          return [blockNumber, header.timestamp] as const;
        }),
      );
      for (const [blockNumber, timestamp] of headers) fetched.set(blockNumber, timestamp);
    }

    return mined.map(([log, m]) => ({
      address: log.address,
      topics: log.topics,
      data: log.data,
      blockNumber: hexToNumber(m.blockNumber),
      blockHash: m.blockHash,
      blockTimestamp: this.timestampOf(log, m, fetched),
      transactionHash: m.transactionHash,
      transactionIndex: hexToNumber(m.transactionIndex),
      logIndex: hexToNumber(m.logIndex),
    }));
  }

  private timestampOf(log: RpcLog, mined: MinedLog, fetched: ReadonlyMap<Hex, number>): number {
    if (log.blockTimestamp !== undefined && log.blockTimestamp !== null) {
      return hexToNumber(log.blockTimestamp);
    }

    const filled = fetched.get(mined.blockNumber);
    // Unreachable: the map is built from exactly the blocks missing a timestamp.
    // Throwing rather than defaulting keeps a broken fill from being written to
    // storage as a plausible-looking 1970 timestamp.
    if (filled === undefined) {
      throw new Error(`no timestamp resolved for block ${mined.blockNumber}`);
    }
    return filled;
  }
}
