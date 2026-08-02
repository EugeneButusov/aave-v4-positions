import type { Address } from '@packages/indexing';

/**
 * What an ERC-20 calls itself, as stored.
 *
 * **Not folded from the ledger.** No Aave event carries a symbol (§12.5), so
 * this is read from the token contract and kept beside the event log rather
 * than derived from it — the first thing here that is enrichment rather than
 * indexed data.
 *
 * **A null label is an answer; a missing row is not.** `symbol` and `name` are
 * OPTIONAL in EIP-20, so a token that has neither is conformant, and the row
 * existing is what records that the question was put. Keeping those two states
 * apart is what stops a mute token being re-read on every sweep forever.
 */
export interface TokenMetadataRow {
  readonly chainId: number;
  /** Lower-cased, so it matches what the Hub fold stored. */
  readonly token: Address;
  readonly symbol: string | null;
  readonly name: string | null;
  /**
   * The token's own `decimals()`, which is **not** what amounts are scaled by.
   *
   * The Hub's `AddAsset` carries its own, that is what the Hub's arithmetic
   * uses, and that is what a position is valued with. This is here so the two
   * can be compared: a disagreement is a listing audit signal, not a
   * correctness problem for our numbers.
   */
  readonly tokenDecimals: number | null;
  /** The block every field above was read at. */
  readonly fetchedAtBlock: number;
}

/** What the read path needs, which is the label and nothing else. */
export interface TokenLabel {
  readonly symbol: string | null;
  readonly name: string | null;
}
