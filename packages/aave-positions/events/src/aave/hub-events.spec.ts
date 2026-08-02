import { getAbiItem, toEventSelector } from 'viem';
import { describe, expect, it } from 'vitest';

import type { DecodedEvent } from '../decode/decoded-event';

import {
  CORE_HUB_ADDRESS,
  CORE_HUB_GENESIS_BLOCK,
  HUB_ABI,
  HUB_STATE_EVENTS,
  HUB_STATE_TOPICS,
  isHubStateEvent,
  listedTokens,
} from './hub-events';
import { MAIN_SPOKE_GENESIS_BLOCK, SPOKE_INGESTED_TOPICS } from './spoke-events';

/**
 * Transcribed from docs/aave-v4-protocol-analysis.md §4.4, which derived them
 * from the Solidity interfaces at commit `2524fe4` and cross-checked them
 * against real Core Hub logs.
 *
 * The point of duplicating them is that they are a *second* derivation. If the
 * address book's ABI and the analysis disagree, one of the two is wrong and
 * this fails — which is a far better outcome than a filter that silently
 * matches nothing.
 */
const ANALYSIS_TOPICS: Readonly<Record<string, string>> = {
  UpdateAsset: '0xa1facf110ded5028ee267fa3d5986f2aa4dc14230b79ffd27e95760f14883350',
  Add: '0xb233dd05ed21346e144167b35a6213bcf04768dbdffdc8339e8b027b94b9f305',
  Remove: '0x535be2ff85ab4c5d0991e10dc057a4951ea2bac426ffb036eded23036a3942b2',
  Draw: '0xe2497bc41b1fa7c4ba996f24dc2affdffb2a5571584db6db0eed8fbbf1dc8517',
  Restore: '0x119e7f996dc987b3ae79eb3735f1620c4292f6a7761a1e0f834c445f7798b912',
  RefreshPremium: '0x3fa96ecf17429fddfbb919a64196f4e43f71b57f0c5c38c49a21c8e1e763d18c',
  ReportDeficit: '0x4845ee5c72bde2b62defc8a1ca2f0fc3313b2d9e799997ce4f6776da9773bcbf',
  EliminateDeficit: '0xe97b8576ac531cdc817b933309d0518ca3d26c6b46d490f3ae9fa39426a141ee',
  MintFeeShares: '0xafd21228e21de4a3f779e1cc3617e12672c3da091dcf3812a931036aa0bf633c',
  Sweep: '0x69bb3893073d7a893f3933f3871309fc25acfc72e365b71f554d439a85b20e8b',
  Reclaim: '0x566111831db1f090374baff3c3f9fc512084f5a9b8f5b199fb475d9c43a8013f',
};

const topicOf = (name: string) =>
  toEventSelector(getAbiItem({ abi: HUB_ABI, name: name as 'Add' }));

describe('Hub event catalogue', () => {
  it('derives the same topic0 the analysis derived, for every event it lists', () => {
    const derived = Object.fromEntries(Object.keys(ANALYSIS_TOPICS).map((n) => [n, topicOf(n)]));

    expect(derived).toEqual(ANALYSIS_TOPICS);
  });

  it('ingests thirteen events, one topic each', () => {
    expect(HUB_STATE_EVENTS).toHaveLength(13);
    expect(new Set(HUB_STATE_TOPICS).size).toBe(13);
  });

  it('leaves out the four that carry no asset state', () => {
    // TransferShares is the one worth pinning: it is a real Hub event that
    // moves shares, and §4.4 shows it moves them *between spokes*, so asset
    // totals net to zero. Folding it would double-count a rebalance.
    for (const name of ['TransferShares', 'AddSpoke', 'UpdateSpokeConfig', 'AuthorityUpdated']) {
      expect(isHubStateEvent(name)).toBe(false);
    }
  });

  it('shares no topic0 with the Spoke, so a merged filter could not confuse them', () => {
    // The topics differ, but the *names* do not — Hub `ReportDeficit` and Spoke
    // `ReportDeficit` are different events (§4.4). That is why the two ledgers
    // are separate tables rather than one filtered by name.
    const shared = HUB_STATE_TOPICS.filter((t) => SPOKE_INGESTED_TOPICS.includes(t));

    expect(shared).toEqual([]);
    expect(isHubStateEvent('ReportDeficit')).toBe(true);
  });

  it('starts the backfill at the Hub, which predates the Spoke', () => {
    // Measured on mainnet. If this ever inverts, the floor belongs to whichever
    // is earlier — the first AddAsset is the only source of `underlying` and
    // `decimals`, and missing it looks like a quiet chain.
    expect(CORE_HUB_GENESIS_BLOCK).toBeLessThan(MAIN_SPOKE_GENESIS_BLOCK);
  });

  it('defaults to a lower-cased address, since log addresses arrive that way', () => {
    expect(CORE_HUB_ADDRESS).toBe('0xcca852bc40e560adc3b1cc58ca5b55638ce826c9');
  });
});

function event(eventName: string, body: Record<string, unknown>): DecodedEvent {
  return {
    chainId: 1,
    address: CORE_HUB_ADDRESS,
    blockNumber: 24_722_784,
    blockHash: `0x${'ab'.repeat(32)}`,
    blockTimestamp: 1_785_000_000,
    txHash: `0x${'cd'.repeat(32)}`,
    txIndex: 0,
    logIndex: 0,
    eventName,
    topic1: null,
    topic2: null,
    topic3: null,
    body,
    data: '0x',
  };
}

describe('listedTokens', () => {
  const USDC = '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48';
  const WETH = '0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2';

  it('takes the underlying off every AddAsset', () => {
    expect(
      listedTokens([
        event('AddAsset', { assetId: '1', underlying: USDC, decimals: 6 }),
        event('AddAsset', { assetId: '2', underlying: WETH, decimals: 18 }),
      ]),
    ).toEqual([USDC, WETH]);
  });

  it('ignores every other Hub event', () => {
    // The claim the whole push rests on: `AddAsset` is the only event that can
    // widen the listed set. `Remove` is a liquidity withdrawal, not a
    // delisting (§4.5), and there is no delisting event at all — so a batch
    // without an `AddAsset` cannot have changed anything.
    const others = HUB_STATE_EVENTS.filter((name) => name !== 'AddAsset');
    expect(others.length).toBeGreaterThan(0);
    expect(others).toContain('Remove');

    expect(listedTokens(others.map((name) => event(name, { underlying: USDC })))).toEqual([]);
  });

  it('lower-cases, because the fold and the store both do', () => {
    const checksummed = '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48';
    expect(listedTokens([event('AddAsset', { underlying: checksummed })])).toEqual([USDC]);
  });

  it('reports one token however many times it was listed', () => {
    // A range re-dispatched after a retry replays the same events, and two
    // asset ids sharing an underlying is not ruled out. Either must be one read.
    expect(
      listedTokens([
        event('AddAsset', { assetId: '1', underlying: USDC }),
        event('AddAsset', { assetId: '2', underlying: USDC }),
      ]),
    ).toEqual([USDC]);
  });

  it('skips a body that carries no usable address', () => {
    // `body` is `Record<string, unknown>` by design. A wrong ABI would put
    // anything here, and writing it to a keyed column is worse than dropping it.
    expect(listedTokens([event('AddAsset', { assetId: '1' })])).toEqual([]);
    expect(listedTokens([event('AddAsset', { underlying: '' })])).toEqual([]);
    expect(listedTokens([event('AddAsset', { underlying: 42 })])).toEqual([]);
  });
});
