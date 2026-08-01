/**
 * A 32-byte hex digest. Declared locally rather than imported from viem so that
 * processors and the reorg detector never take a type dependency on the RPC
 * library; it is structurally identical to viem's own `Hash`.
 */
export type Hash = `0x${string}`;

/**
 * The only part of a block the framework itself needs: enough to establish
 * ancestry. Nothing here describes transactions or logs — that is a processor's
 * business, and the port stays narrow until one actually needs it.
 */
export interface BlockHeader {
  readonly number: number;
  readonly hash: Hash;
  readonly parentHash: Hash;
  /** Unix seconds, as reported by the block itself. */
  readonly timestamp: number;
}

/**
 * The chain reads the indexing loop depends on. Deliberately three methods: a
 * narrow port is what makes the loop testable against a scripted chain, and
 * every method added here has to be re-implemented by every fake.
 *
 * Block numbers are plain `number`, not `bigint`. They are the framework's
 * arithmetic domain — compared, subtracted and used as map keys on every
 * iteration — and stay far below `Number.MAX_SAFE_INTEGER`. This is unrelated
 * to the `uint256`-as-string rule that applies to balances: those really do
 * exceed float64, block heights do not.
 */
export interface ChainClient {
  /** The chain the configured providers actually serve. */
  getChainId(): Promise<number>;
  getHeadBlockNumber(): Promise<number>;
  getBlockHeader(blockNumber: number): Promise<BlockHeader>;
}

export const CHAIN_CLIENT = Symbol('CHAIN_CLIENT');

/**
 * What a {@link ChainClient} adapter needs to reach a chain. Deliberately not
 * the indexing options: this layer knows about providers and timeouts, not
 * about cursors, finality or processors.
 */
export interface ChainClientOptions {
  /** Tried in order. The first is the preferred provider, not merely one of many. */
  readonly rpcUrls: readonly string[];
  readonly rpcTimeoutMs: number;
}

export const CHAIN_CLIENT_OPTIONS = Symbol('CHAIN_CLIENT_OPTIONS');
