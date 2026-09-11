import type { Address } from '@packages/indexing';

import type { HubAsset } from './hub-asset';

/**
 * Reads the Hub asset fold.
 *
 * **No pagination, deliberately.** A Hub lists 17 assets on mainnet and 34
 * across all four; the whole dimension fits in one response and the valuation
 * path wants all of it at once, so a cursor would be ceremony over a table that
 * cannot grow into needing one. If a Hub ever lists thousands, this gains the
 * same keyset shape `PositionStore` uses.
 */
export interface HubAssetStore {
  /**
   * Every asset on one Hub, ordered by asset id, as it stood at `asOf`.
   *
   * **The instant is optional and defaults to now, but a caller comparing
   * against the chain should name it.** The Hub's totals and its interest
   * checkpoint both move with every event, so reading them at now and comparing
   * against a contract call pinned to a block reports the fold being ahead as
   * though it were drift. Unix seconds, matching `PositionQuery.asOf`.
   */
  list(chainId: number, hub: Address, asOf?: bigint): Promise<readonly HubAsset[]>;

  /** One asset as it stood at `asOf`, or null if the Hub had not listed it. */
  get(chainId: number, hub: Address, assetId: string, asOf?: bigint): Promise<HubAsset | null>;
}

export const HUB_ASSET_STORE = Symbol('HUB_ASSET_STORE');
