import { IHubV4_ABI } from '@aave-dao/aave-address-book/abis';
import { AaveV4Ethereum } from '@aave-dao/aave-address-book';
import type { Hash } from '@packages/indexing';
import { getAbiItem, toEventSelector } from 'viem';

/**
 * The Hub ABI, taken from the official address book rather than transcribed.
 *
 * Every topic0 derived from it was checked against the catalogue in
 * docs/aave-v4-protocol-analysis.md §4.4, which was extracted independently
 * from the Solidity interfaces. All twelve hashes the analysis lists agree —
 * see `hub-events.spec.ts`, which asserts it rather than leaving it to a
 * comment.
 */
export const HUB_ABI = IHubV4_ABI;

/**
 * The events that move Hub asset state, and so the inputs to the share→asset
 * conversion of §5.
 *
 * Ten of them move quantities that are **additive**; three set a value that is
 * **latest-wins**. That split is what the fold's two tables are built on, but
 * it does not matter here — this list exists to build the log filter.
 *
 * Deliberately not the whole ABI:
 *
 * - `TransferShares` is excluded because it provably changes nothing at asset
 *   grain. §4.4, verified: it moves `addedShares` between two `SpokeData`
 *   records, so asset-level totals net to zero. It is inter-spoke rebalancing,
 *   and it is how the liquidation fee settles to the treasury spoke — relevant
 *   only to per-spoke subtotals, which nothing here keeps.
 * - `AddSpoke`, `UpdateSpokeConfig` and `AuthorityUpdated` carry no asset
 *   state. Unlike the Spoke's `AddReserve` — ingested with no projection
 *   because a later increment needs it — nothing downstream has a use for
 *   these.
 */
export const HUB_STATE_EVENTS = [
  // Additive: liquidity, shares, premium and deficit.
  'Add',
  'Remove',
  'Draw',
  'Restore',
  'MintFeeShares',
  'Sweep',
  'Reclaim',
  'ReportDeficit',
  'EliminateDeficit',
  'RefreshPremium',
  // Latest-wins.
  'UpdateAsset', // the interest checkpoint — §5.3, the reason none of this needs an RPC
  'UpdateAssetConfig', // carries liquidityFee, which the supply-side fee term needs
  'AddAsset', // underlying + decimals, available from no other event
] as const;

export type HubStateEvent = (typeof HUB_STATE_EVENTS)[number];

/**
 * `topic0` for each of the above, computed from the ABI at load rather than
 * pasted in. A hardcoded hash cannot drift silently; a wrong one just matches
 * nothing, which looks exactly like a quiet chain.
 */
export const HUB_STATE_TOPICS: readonly Hash[] = HUB_STATE_EVENTS.map((name) =>
  toEventSelector(getAbiItem({ abi: HUB_ABI, name })),
);

const HUB_STATE_EVENT_NAMES: ReadonlySet<string> = new Set(HUB_STATE_EVENTS);

export function isHubStateEvent(name: string): name is HubStateEvent {
  return HUB_STATE_EVENT_NAMES.has(name);
}

/**
 * Core Hub on Ethereum mainnet, and the default for `CORE_HUB_ADDRESS`.
 *
 * A default, not a constant the code reaches for. All 14 of the Main Spoke's
 * reserves point at this Hub, so the pair is self-contained and needs no
 * cross-hub read (§2, verified) — a second Hub is a second registration.
 */
export const CORE_HUB_ADDRESS = AaveV4Ethereum.HUBS.CORE_HUB.toLowerCase();

/**
 * Earliest block emitting Core Hub logs, and so the backfill floor.
 *
 * **Eight blocks before the Main Spoke's** (24,720,899), which is why
 * `INDEXER_START_BLOCK` defaults to this one rather than the Spoke's. Those
 * eight blocks hold only the proxy's own lifecycle events — `Upgraded`,
 * `Initialized`, `AuthorityUpdated`, `AdminChanged` — none of which are
 * ingested, so starting at the Spoke's genesis happens to lose nothing today.
 * Depending on that coincidence is the problem: the first `AddAsset` is the
 * only source of `underlying` and `decimals`, it fires once per asset, and
 * missing it looks like a quiet chain rather than an error.
 *
 * Measured against mainnet: first Core Hub log at 24,720,891, first *state*
 * event at 24,722,784, where all 17 `AddAsset` fire in one block.
 */
export const CORE_HUB_GENESIS_BLOCK = 24_720_891;
