import type { Address } from './chain-client';

/**
 * What an ERC-20 says about itself.
 *
 * **Every field is nullable, because every field is optional.** EIP-20 marks
 * `name()` and `symbol()` OPTIONAL, and `decimals()` with them — a token that
 * implements none of the three is unusual but conformant. A null here is an
 * answer, not a failure to get one.
 *
 * `decimals` is the token's own and is deliberately not the number anything is
 * scaled by: the Hub's `AddAsset` carries its own, that is what the Hub's
 * arithmetic uses, and that is what a position is valued with. This one exists
 * so the two can be compared.
 */
export interface TokenMetadata {
  readonly symbol: string | null;
  readonly name: string | null;
  readonly decimals: number | null;
  /**
   * Why a field is null, one entry per field that could not be read.
   *
   * Reported rather than thrown, because "this token has no symbol" and "this
   * token could not be reached" are both routine and the caller has to tell
   * them apart. Empty when every field resolved.
   */
  readonly failures: readonly string[];
}

/**
 * ERC-20 metadata reads, kept apart from {@link ChainClient}.
 *
 * A fourth port rather than a fourth method on the first one, for the reason
 * {@link LogReader} gives: `ChainClient` is what the indexing loop consumes, and
 * every fake of it would have to grow a method the loop never calls.
 *
 * **An ERC-20 in a package that knows nothing about Aave** is deliberate.
 * ERC-20 is an Ethereum standard, exactly as `eth_getLogs` is an Ethereum
 * method; neither is a protocol this framework is supposed to know about. What
 * would not belong here is anything that knew *which* tokens to read.
 */
export interface Erc20MetadataReader {
  /**
   * Reads one token at a pinned block.
   *
   * Pinned rather than at `latest`, because three calls at `latest` across a
   * provider failover list are three calls at possibly three heights. Metadata
   * is immutable in practice, so this is about the reads agreeing with each
   * other rather than about history.
   */
  read(token: Address, atBlock: bigint): Promise<TokenMetadata>;
}

export const ERC20_METADATA_READER = Symbol('ERC20_METADATA_READER');
