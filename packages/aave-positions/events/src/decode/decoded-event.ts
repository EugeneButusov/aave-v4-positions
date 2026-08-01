import type { Address, Hash, Hex } from '@packages/indexing';

/**
 * A decoded log, ready to store.
 *
 * One shape for all eight events, which is only possible because the variable
 * part is not spread across columns: the indexed parameters stay the topic
 * words they arrived as, and everything else is JSON. Only three of the eight
 * share a field layout, so a column per field would be mostly nulls and would
 * need a migration per new event.
 *
 * Carries the raw envelope beside the decoded `body`. The duplication is
 * deliberate: it roughly doubles the row, which is nothing at this volume after
 * columnar compression, and it means a decoder bug is repairable by re-decoding
 * what is already stored rather than re-fetching the whole backfill from a
 * rate-limited provider. §9.4's "repair is replay, not patch", applied one level
 * below the position fold.
 */
export interface DecodedEvent {
  readonly chainId: number;
  /** The emitting Spoke, lower-cased. Which ABI decoded this log (§4.5). */
  readonly address: Address;
  readonly blockNumber: number;
  readonly blockHash: Hash;
  /** Unix seconds. §5.3 needs this as the interest-checkpoint timestamp. */
  readonly blockTimestamp: number;
  readonly txHash: Hash;
  readonly txIndex: number;
  readonly logIndex: number;
  readonly eventName: string;

  /**
   * The indexed parameters, in ABI order, as the 32-byte words they arrived as.
   * `topic0` is the signature hash, which {@link eventName} already says.
   *
   * **Only `topic1` means the same thing across all eight** — the reserve id,
   * or the collateral one on `LiquidationCall`. `topic2` is `caller`, `user`,
   * `debtReserveId` or `assetId` depending on the event, and `topic3` is `user`
   * or `hub`, or absent on `ReportDeficit`. Reading either without knowing
   * `eventName` is how a query quietly returns the wrong field, which is why
   * the fold reads one view per event rather than the table.
   */
  readonly topic1: Hash | null;
  readonly topic2: Hash | null;
  readonly topic3: Hash | null;

  /**
   * Every decoded argument, indexed and not, with each `uint256` already a
   * string so the row survives `JSON.stringify` and §7.5 is not walked into.
   */
  readonly body: Readonly<Record<string, unknown>>;

  /** The raw non-indexed bytes, so `body` can be re-derived without the chain. */
  readonly data: Hex;
}
