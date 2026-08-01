import type { BlockHeader, ChainClient, Hash } from '../chain/chain-client';

export type ChainCall =
  | { readonly method: 'getChainId' }
  | { readonly method: 'getHeadBlockNumber' }
  | { readonly method: 'getBlockHeader'; readonly blockNumber: number };

const ZERO_HASH: Hash = `0x${'0'.repeat(64)}`;

/**
 * Deterministic hash for a block on a named branch. Branch tags must be hex
 * characters so the result stays a valid hash — 'a' and 'b' by convention.
 */
export function hashOf(branch: string, blockNumber: number): Hash {
  if (blockNumber < 0) return ZERO_HASH;
  return `0x${`${branch}${blockNumber}`.padStart(64, '0')}`;
}

/**
 * A scripted chain. Serves headers from a synthetic linear history, can be
 * re-pointed onto a different branch above any block to simulate a fork, and
 * records every call so tests can assert what was *not* asked for.
 */
export class FakeChainClient implements ChainClient {
  readonly calls: ChainCall[] = [];

  private chainId: number;
  private head: number;
  private readonly forks: { above: number; branch: string }[] = [];
  private pendingFailures = 0;
  private failure: Error = new Error('rpc unavailable');

  constructor(options: { chainId?: number; head?: number } = {}) {
    this.chainId = options.chainId ?? 1;
    this.head = options.head ?? 0;
  }

  setHead(blockNumber: number): this {
    this.head = blockNumber;
    return this;
  }

  setChainId(chainId: number): this {
    this.chainId = chainId;
    return this;
  }

  /** Everything strictly above `forkPoint` now belongs to `branch`. */
  forkAbove(forkPoint: number, branch: string): this {
    this.forks.push({ above: forkPoint, branch });
    return this;
  }

  /** The next `count` calls of any kind reject. */
  failNext(count: number, error = new Error('rpc unavailable')): this {
    this.pendingFailures = count;
    this.failure = error;
    return this;
  }

  branchAt(blockNumber: number): string {
    let branch = 'a';
    for (const fork of this.forks) {
      if (blockNumber > fork.above) branch = fork.branch;
    }
    return branch;
  }

  headerAt(blockNumber: number): BlockHeader {
    return {
      number: blockNumber,
      hash: hashOf(this.branchAt(blockNumber), blockNumber),
      parentHash: hashOf(this.branchAt(blockNumber - 1), blockNumber - 1),
      timestamp: 1_700_000_000 + blockNumber * 12,
    };
  }

  /** Calls recorded for one method, in order. */
  callsTo(method: ChainCall['method']): ChainCall[] {
    return this.calls.filter((call) => call.method === method);
  }

  getChainId(): Promise<number> {
    this.calls.push({ method: 'getChainId' });
    return this.settle(this.chainId);
  }

  getHeadBlockNumber(): Promise<number> {
    this.calls.push({ method: 'getHeadBlockNumber' });
    return this.settle(this.head);
  }

  getBlockHeader(blockNumber: number): Promise<BlockHeader> {
    this.calls.push({ method: 'getBlockHeader', blockNumber });
    return this.settle(this.headerAt(blockNumber));
  }

  private settle<T>(value: T): Promise<T> {
    if (this.pendingFailures > 0) {
      this.pendingFailures -= 1;
      return Promise.reject(this.failure);
    }
    return Promise.resolve(value);
  }
}
