import type { Address } from '@packages/indexing';

import type { HubAsset } from './hub-asset';

/**
 * Reads the Hub asset mirror.
 *
 * **No pagination, deliberately.** A Hub lists 17 assets on mainnet and 34
 * across all four; the whole dimension fits in one response and the valuation
 * path wants all of it at once, so a cursor would be ceremony over a table that
 * cannot grow into needing one. If a Hub ever lists thousands, this gains the
 * same keyset shape `PositionStore` uses.
 */
export interface HubAssetStore {
  /** Every asset on one Hub, ordered by asset id. */
  list(chainId: number, hub: Address): Promise<readonly HubAsset[]>;

  /** One asset, or null if the Hub has never listed it. */
  get(chainId: number, hub: Address, assetId: string): Promise<HubAsset | null>;
}

export const HUB_ASSET_STORE = Symbol('HUB_ASSET_STORE');
