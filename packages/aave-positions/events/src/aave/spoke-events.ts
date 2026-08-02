import { ISpokeV4_ABI } from '@aave-dao/aave-address-book/abis';
import { AaveV4Ethereum } from '@aave-dao/aave-address-book';
import type { Hash } from '@packages/indexing';
import { getAbiItem, toEventSelector } from 'viem';

/**
 * The Spoke ABI, taken from the official address book rather than transcribed.
 *
 * Every topic0 derived from it was checked against the catalogue in
 * docs/aave-v4-protocol-analysis.md §4, which was extracted independently from
 * the Solidity interfaces and cross-checked against 38,580 real Main Spoke logs.
 * Two independent derivations agreeing is what makes this a trustworthy source;
 * the analysis also notes that deriving `Repay` by hand is an easy way to get a
 * signature subtly wrong and match nothing.
 */
export const SPOKE_ABI = ISpokeV4_ABI;

/**
 * The events that move a position, plus the registry that gives `reserveId` a
 * meaning. Exactly §12.2's fold inputs.
 *
 * Deliberately **not** the whole ABI. Config and dynamic-config events, and the
 * Hub's own stream, are later increments. The position managers' mirror events
 * (`SupplyOnBehalfOf` and friends) are excluded permanently: §2 shows they are
 * provenance records for actions the Spoke has already reported, so folding
 * them alongside would double-count every routed action.
 */
export const SPOKE_POSITION_EVENTS = [
  'Supply',
  'Withdraw',
  'Borrow',
  'Repay',
  'LiquidationCall',
  'ReportDeficit',
  'SetUsingAsCollateral',
  'AddReserve',
] as const;

export type SpokePositionEvent = (typeof SPOKE_POSITION_EVENTS)[number];

/**
 * `topic0` for each of the above, computed from the ABI at load rather than
 * pasted in. A hardcoded hash cannot drift silently; a wrong one just matches
 * nothing, which looks exactly like a quiet chain.
 */
export const SPOKE_POSITION_TOPICS: readonly Hash[] = SPOKE_POSITION_EVENTS.map((name) =>
  toEventSelector(getAbiItem({ abi: SPOKE_ABI, name })),
);

const POSITION_EVENT_NAMES: ReadonlySet<string> = new Set(SPOKE_POSITION_EVENTS);

export function isPositionEvent(name: string): name is SpokePositionEvent {
  return POSITION_EVENT_NAMES.has(name);
}

/**
 * Main Spoke on Ethereum mainnet, and the default for `MAIN_SPOKE_ADDRESS`.
 *
 * A default, not a constant the code reaches for: the processor takes its Spoke
 * as configuration, so a second Spoke is a second registration rather than an
 * edit here.
 */
export const MAIN_SPOKE_ADDRESS = AaveV4Ethereum.SPOKES.MAIN_SPOKE.toLowerCase();

/**
 * The oracle that prices the Main Spoke's reserves, and the default for
 * `MAIN_SPOKE_ORACLE_ADDRESS`.
 *
 * **Paired with the Spoke above rather than listed as a chain-level address**,
 * because that is what it is: `IAaveOracleV4` belongs to one Spoke and indexes
 * by `reserveId` (§7.4), so a second Spoke brings a second oracle over a
 * disjoint id space. Reading a reserve id against the wrong one would answer
 * with a price rather than an error.
 *
 * Taken from the address book, and safe to take from there: §7.4 probed
 * `0x99B2…6127` independently and reached the same value, which is the same
 * two-derivations-agreeing argument {@link SPOKE_ABI} rests on.
 */
export const MAIN_SPOKE_ORACLE_ADDRESS = AaveV4Ethereum.SPOKES.MAIN_SPOKE_ORACLE.toLowerCase();

/** Earliest block emitting Main Spoke logs (§2), and so the backfill floor. */
export const MAIN_SPOKE_GENESIS_BLOCK = 24_720_899;
