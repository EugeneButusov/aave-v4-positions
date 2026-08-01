import type { Address, Hash, Hex } from './chain-client';

/**
 * One log, as the chain reported it.
 *
 * Undecoded on purpose: the framework has no opinion about what any of these
 * events mean. A processor pairs `address` with an ABI and decodes; this layer
 * only fetches.
 *
 * Numbers are `number` for the same reason as in {@link BlockHeader} — heights,
 * indexes and Unix seconds are the framework's arithmetic domain and stay far
 * below `Number.MAX_SAFE_INTEGER`. The `uint256`-as-string rule applies to what
 * is inside `data`, which stays hex here and is a processor's problem.
 */
export interface RawLog {
  readonly address: Address;
  /** `topics[0]` is the event signature hash, except for anonymous events. */
  readonly topics: readonly Hash[];
  readonly data: Hex;
  readonly blockNumber: number;
  readonly blockHash: Hash;
  /**
   * Unix seconds of the containing block.
   *
   * Guaranteed by the adapter, not by the wire: `eth_getLogs` returns
   * `blockTimestamp` on some providers and omits it on others. An adapter that
   * cannot get it from the response has to go and fetch it, so that callers
   * never carry a provider's quirk into their own code.
   */
  readonly blockTimestamp: number;
  readonly transactionHash: Hash;
  readonly transactionIndex: number;
  /**
   * Position within the **block**, not within the transaction — verified against
   * mainnet, where one block's 838 logs ran 0..837 unbroken across transaction
   * boundaries. That is what makes `(blockNumber, logIndex)` a unique key.
   */
  readonly logIndex: number;
}

export interface LogFilter {
  readonly addresses: readonly Address[];
  /** Matches any of these event signatures. Empty means no topic constraint. */
  readonly topic0: readonly Hash[];
  /** Inclusive on both ends, matching `eth_getLogs`. */
  readonly fromBlock: number;
  readonly toBlock: number;
}

/**
 * Log reads, kept apart from {@link ChainClient}.
 *
 * A second port rather than a fourth method on the first one: `ChainClient` is
 * what the indexing loop itself consumes, and every fake of it would have to
 * grow a `getLogs` the loop never calls. Nothing in the loop reads logs — only
 * processors do.
 */
export interface LogReader {
  getLogs(filter: LogFilter): Promise<RawLog[]>;
}

export const LOG_READER = Symbol('LOG_READER');

/**
 * The provider refused the block span, and a narrower one may succeed.
 *
 * Distinct from an ordinary RPC failure because the response differs: a
 * processor turns this into `retry(..., { narrowRange: true })` so the loop
 * halves its range, where any other error is just a retry at the same width.
 * Measured caps across public endpoints range from 50 blocks to 50,000, so this
 * is a routine condition rather than an exceptional one.
 */
export class LogRangeTooLargeError extends Error {
  constructor(
    readonly fromBlock: number,
    readonly toBlock: number,
    readonly detail: string,
  ) {
    super(`provider rejected the range ${fromBlock}..${toBlock}: ${detail}`);
    this.name = 'LogRangeTooLargeError';
  }
}
