import type { DecodedEvent } from '@aave-positions/events';

import { CHAIN_ID, SPOKE, type At } from './spoke-ledger';

/** Core Hub on mainnet, lower-cased as a log's address arrives. */
export const HUB = '0xcca852bc40e560adc3b1cc58ca5b55638ce826c9';
/** USDC, and the `underlying` the AddAsset builder reports. */
export const USDC = '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48';
/** RAY, which is where `drawnIndex` starts and the floor it never goes below. */
export const RAY = '1000000000000000000000000000';

/** Tables a Hub suite truncates between tests. */
export const HUB_TABLES = ['hub_events', 'hub_assets', 'hub_asset_state'];

/** A decoded Hub log, shaped as the decoder would hand it over. */
export function hubEvent(name: string, at: At, body: Record<string, unknown>): DecodedEvent {
  return {
    chainId: CHAIN_ID,
    address: HUB,
    blockNumber: at.block,
    blockHash: `0x${'aa'.repeat(32)}`,
    // Distinct per block, so `index_timestamp` can be checked against the
    // checkpoint it belongs to rather than a constant.
    blockTimestamp: 1_785_000_000 + at.block,
    txHash: `0x${'bb'.repeat(32)}`,
    txIndex: 0,
    logIndex: at.log ?? 0,
    eventName: name,
    topic1: null,
    topic2: null,
    topic3: null,
    body,
    data: '0x',
  };
}

const ASSET = '7';

/** No premium movement, which is every Restore and ReportDeficit on mainnet (§5.4). */
export const NO_PREMIUM_DELTA = {
  sharesDelta: '0',
  offsetRayDelta: '0',
  restoredPremiumRay: '0',
};

export const add = (at: At, shares: string, amount: string, assetId = ASSET) =>
  hubEvent('Add', at, { assetId, spoke: SPOKE, shares, amount });

export const remove = (at: At, shares: string, amount: string, assetId = ASSET) =>
  hubEvent('Remove', at, { assetId, spoke: SPOKE, shares, amount });

export const draw = (at: At, drawnShares: string, drawnAmount: string, assetId = ASSET) =>
  hubEvent('Draw', at, { assetId, spoke: SPOKE, drawnShares, drawnAmount });

export const restore = (
  at: At,
  drawnShares: string,
  drawnAmount: string,
  premiumAmount = '0',
  premiumDelta = NO_PREMIUM_DELTA,
  assetId = ASSET,
) =>
  hubEvent('Restore', at, {
    assetId,
    spoke: SPOKE,
    drawnShares,
    premiumDelta,
    drawnAmount,
    premiumAmount,
  });

export const reportDeficit = (
  at: At,
  drawnShares: string,
  deficitAmountRay: string,
  premiumDelta = NO_PREMIUM_DELTA,
  assetId = ASSET,
) =>
  hubEvent('ReportDeficit', at, {
    assetId,
    spoke: SPOKE,
    drawnShares,
    premiumDelta,
    deficitAmountRay,
  });

export const eliminateDeficit = (
  at: At,
  shares: string,
  deficitAmountRay: string,
  assetId = ASSET,
) =>
  hubEvent('EliminateDeficit', at, {
    assetId,
    callerSpoke: SPOKE,
    coveredSpoke: SPOKE,
    shares,
    deficitAmountRay,
  });

export const mintFeeShares = (at: At, shares: string, assets: string, assetId = ASSET) =>
  hubEvent('MintFeeShares', at, { assetId, feeReceiver: SPOKE, shares, assets });

export const sweep = (at: At, amount: string, assetId = ASSET) =>
  hubEvent('Sweep', at, { assetId, reinvestmentController: SPOKE, amount });

export const reclaim = (at: At, amount: string, assetId = ASSET) =>
  hubEvent('Reclaim', at, { assetId, reinvestmentController: SPOKE, amount });

export const refreshPremium = (
  at: At,
  sharesDelta: string,
  offsetRayDelta: string,
  assetId = ASSET,
) =>
  hubEvent('RefreshPremium', at, {
    assetId,
    spoke: SPOKE,
    premiumDelta: { sharesDelta, offsetRayDelta, restoredPremiumRay: '0' },
  });

export const updateAsset = (
  at: At,
  drawnIndex = RAY,
  drawnRate = '0',
  accruedFees = '0',
  assetId = ASSET,
) => hubEvent('UpdateAsset', at, { assetId, drawnIndex, drawnRate, accruedFees });

export const updateAssetConfig = (at: At, liquidityFee: number, assetId = ASSET) =>
  hubEvent('UpdateAssetConfig', at, {
    assetId,
    config: {
      feeReceiver: SPOKE,
      liquidityFee,
      irStrategy: SPOKE,
      reinvestmentController: SPOKE,
    },
  });

export const addAsset = (at: At, underlying = USDC, decimals = 6, assetId = ASSET) =>
  hubEvent('AddAsset', at, { assetId, underlying, decimals });
