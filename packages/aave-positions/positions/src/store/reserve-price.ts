import type { Address } from '@packages/indexing';

/**
 * What Aave prices one reserve at, as stored.
 *
 * **Not folded from the ledger.** No Aave event carries a price (§4.4), so this
 * is read from the Spoke's own oracle and kept beside the event log rather than
 * derived from it — enrichment in the same sense token metadata is, and the
 * second thing here that is fetched rather than indexed.
 *
 * **Keyed by reserve, not by token.** `IAaveOracleV4` indexes by `reserveId`
 * and belongs to one Spoke, so the same ERC-20 listed on two Spokes has two
 * prices and they are allowed to disagree — each Spoke is an isolated margin
 * account with its own oracle (§12.3). A price keyed by token address would
 * quietly pick one of them.
 */
export interface ReservePriceRow {
  readonly chainId: number;
  /** Lower-cased, so it matches what the position fold stored. */
  readonly spoke: Address;
  /** Decimal digits, no leading zeros — one spelling per reserve. */
  readonly reserveId: string;
  /**
   * 8-decimal, per §7.4's `ORACLE_DECIMALS`.
   *
   * A decimal string rather than a number: §7.1 multiplies this by an amount to
   * reach a `Value` where `1e26` is one dollar, and float64 would round the tail
   * off both operands long before the product mattered (§7.5).
   */
  readonly price: string;
}

/** A stored price, the moment it was read, and how long ago that was. */
export interface ReservePrice {
  readonly price: string;
  /**
   * Written by the database's clock, so freshness is judged against the server
   * that wrote it rather than the one reading it.
   *
   * **Not a block.** The read that produced this was pinned to one, but the
   * store is shared with whatever source lands next and a market API has no
   * block to give — see `011_reserve_prices.sql`.
   */
  readonly pricedAt: Date;
  /**
   * Whole seconds since this price was written, **measured by the database's
   * own clock**.
   *
   * Not derivable from {@link pricedAt} by a reader, for the reason
   * {@link SyncStatus.ageSeconds} gives: that timestamp is written by the
   * database, so subtracting it from a different process's clock reports skew
   * as staleness — and in the direction that matters, a fast reader declares a
   * fresh price stale and a whole page's USD values suspect.
   */
  readonly ageSeconds: number;
}
