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
 * The events that decide what a position *counts as*, rather than what is in it.
 *
 * None of them moves a share balance, and that is exactly why they are easy to
 * leave out and wrong to. §7.1 weights collateral by `collateralFactor` at the
 * user's pinned `dynamicConfigKey` — `_dynamicConfig[reserveId][key]`, not the
 * reserve's current one (§3) — so a health factor without these is not an
 * approximation, it is unavailable. §12.1's position *type* needs the same
 * versioned config plus the reserve flags.
 *
 * **Two of them carry the version and two of them do not**, which is the whole
 * difficulty and is dealt with where the fold is, not here.
 * `Add`/`UpdateDynamicReserveConfig` name a `dynamicConfigKey` and the values
 * under it; the two refresh events say only that a user moved to whatever the
 * reserve's key was *at that block*, so resolving one needs the other's history.
 *
 * `RefreshAllUserDynamicConfig` is the **most frequent event on the Spoke** —
 * 8,696 against `Supply`'s fewer (§4.2). It moves no value and still has to be
 * ingested, which is the clearest statement of why "position events" was never
 * the same set as "events the position layer needs".
 */
export const SPOKE_CONFIG_EVENTS = [
  'AddDynamicReserveConfig',
  'UpdateDynamicReserveConfig',
  'RefreshAllUserDynamicConfig',
  'RefreshSingleUserDynamicConfig',
  'UpdateReserveConfig',
] as const;

export type SpokeConfigEvent = (typeof SPOKE_CONFIG_EVENTS)[number];

/**
 * Everything read out of the Spoke, which is the union of the two groups above.
 *
 * Kept as one list rather than two call sites, because the topic filter and the
 * decoder's allow-list have to agree exactly: a topic requested but not decoded
 * throws on arrival, and a name decoded but not requested never arrives.
 *
 * **Widening this list requires a backfill.** The ledger holds what the filter
 * asked for at the time it was written, so history has no rows for an event
 * added later — and nothing detects that, because a fold over the missing rows
 * produces a smaller number rather than an error.
 */
export const SPOKE_INGESTED_EVENTS = [...SPOKE_POSITION_EVENTS, ...SPOKE_CONFIG_EVENTS] as const;

export type SpokeIngestedEvent = SpokePositionEvent | SpokeConfigEvent;

/**
 * `topic0` for each of the above, computed from the ABI at load rather than
 * pasted in. A hardcoded hash cannot drift silently; a wrong one just matches
 * nothing, which looks exactly like a quiet chain.
 */
export const SPOKE_INGESTED_TOPICS: readonly Hash[] = SPOKE_INGESTED_EVENTS.map((name) =>
  toEventSelector(getAbiItem({ abi: SPOKE_ABI, name })),
);

const INGESTED_EVENT_NAMES: ReadonlySet<string> = new Set<string>(SPOKE_INGESTED_EVENTS);

export function isIngestedSpokeEvent(name: string): name is SpokeIngestedEvent {
  return INGESTED_EVENT_NAMES.has(name);
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
