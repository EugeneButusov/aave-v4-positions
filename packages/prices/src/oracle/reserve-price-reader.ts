import type { Address } from '@packages/indexing';

/**
 * What a Spoke's oracle says its reserves are worth.
 *
 * **Absent, never null.** A reserve the oracle refused is simply not in
 * {@link prices}, and the reason is in {@link failures}. There is no null price
 * to represent: the oracle reverts rather than answer zero (§7.4), so a zero
 * reaching a caller could only ever be this code's own invention — and storing
 * one would close over a gap that a later read would have filled.
 */
export interface ReservePrices {
  /**
   * The block every price here was read at.
   *
   * Diagnostic, and deliberately does not reach the store. Pinning matters
   * because fourteen reads across a provider failover list are otherwise
   * fourteen reads at up to fourteen heights; what to *persist* is a timestamp,
   * because the next price source may have no block at all
   * (`011_reserve_prices.sql`).
   */
  readonly blockNumber: bigint;
  /**
   * `reserveId` to its 8-decimal price, both as decimal strings.
   *
   * Strings rather than `bigint`, for the reason `ViemChainClient` gives about
   * block numbers: the conversion out of viem's types stops at the adapter, so
   * nothing downstream has to think about it. §7.5's rule points the same way —
   * a price is multiplied by an amount to reach a `Value`, and float64 would
   * round the tail off both operands.
   */
  readonly prices: ReadonlyMap<string, string>;
  /**
   * Why a reserve is absent, one entry per reserve that could not be read.
   *
   * Reported rather than thrown, because "this one reserve has no usable price"
   * and "the oracle could not be reached" are different situations and the
   * caller has to tell them apart. Empty when every price resolved.
   */
  readonly failures: readonly string[];
}

/**
 * Reserve price reads, kept apart from {@link ChainClient}.
 *
 * **In this package rather than in `@packages/indexing`**, which is the rule
 * `Erc20MetadataReader` states from the other side: ERC-20 is an Ethereum
 * standard and belongs in a package that knows nothing about Aave, while
 * *"anything that knew which tokens to read"* does not. An Aave oracle is
 * entirely that — it is keyed by `reserveId`, it belongs to one Spoke, and its
 * ABI is protocol knowledge.
 */
export interface ReservePriceReader {
  /**
   * Reads a whole Spoke's prices at a pinned block.
   *
   * Pinned rather than at `latest` for the reason {@link Erc20MetadataReader}
   * gives, and one more: §7.1's arithmetic weighs collateral against debt, so
   * two prices from two heights do not merely disagree — they misprice the
   * ratio the health factor *is*.
   */
  read(oracle: Address, reserveIds: readonly string[], atBlock: bigint): Promise<ReservePrices>;
}

export const RESERVE_PRICE_READER = Symbol('RESERVE_PRICE_READER');
