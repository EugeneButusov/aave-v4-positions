import { getAbiItem, toEventSelector } from 'viem';
import { describe, expect, it } from 'vitest';

import {
  MAIN_SPOKE_ADDRESS,
  MAIN_SPOKE_ORACLE_ADDRESS,
  SPOKE_ABI,
  SPOKE_CONFIG_EVENTS,
  SPOKE_INGESTED_EVENTS,
  SPOKE_INGESTED_TOPICS,
  SPOKE_POSITION_EVENTS,
  isIngestedSpokeEvent,
} from './spoke-events';

/**
 * Transcribed from docs/aave-v4-protocol-analysis.md §4.1–§4.3, which derived
 * them from the Solidity interfaces at commit `2524fe4` and cross-checked them
 * against 38,580 real Main Spoke logs.
 *
 * The point of duplicating them is that they are a *second* derivation. If the
 * address book's ABI and the analysis disagree, one of the two is wrong and
 * this fails — which is a far better outcome than a filter that silently
 * matches nothing, and is exactly the shape a subtly wrong `Repay` signature
 * would take.
 */
const ANALYSIS_TOPICS: Readonly<Record<string, string>> = {
  AddReserve: '0xb2d3221c3db1eb0d586556ae23399acdfe3e52ff0fcd184c19069c730f9ca2e9',
  AddDynamicReserveConfig: '0xfcede5501ba87e3766118ae6ed360a87ee9b6570156ae9cac52d35ff0de0403b',
  UpdateDynamicReserveConfig: '0x2d4f2760aaff0dfa53526a8fdd306864689a7d5e43f44ddfeece0f38315c298d',
  RefreshAllUserDynamicConfig: '0x837314749a8459031ad895d39a13552d1627fddc93d64b404bab0ae5f0798da7',
  RefreshSingleUserDynamicConfig:
    '0x5790b5f096c9cfee6b98a4e2d4f54ff3fc4ca306df5bc2093d93a36496d917b8',
  UpdateReserveConfig: '0xe9495512a0eb05fe0cbdd52286bdeb54cb8e5a8d50e7e17d75f75903a98e2af8',
};

const topicOf = (name: string) =>
  toEventSelector(getAbiItem({ abi: SPOKE_ABI, name: name as 'AddReserve' }));

describe('Spoke event catalogue', () => {
  it('derives the same topic0 the analysis derived, for every event it lists', () => {
    const derived = Object.fromEntries(Object.keys(ANALYSIS_TOPICS).map((n) => [n, topicOf(n)]));

    expect(derived).toEqual(ANALYSIS_TOPICS);
  });

  it('ingests thirteen events, one topic each', () => {
    expect(SPOKE_POSITION_EVENTS).toHaveLength(8);
    expect(SPOKE_CONFIG_EVENTS).toHaveLength(5);
    expect(SPOKE_INGESTED_EVENTS).toHaveLength(13);
    // A duplicate would be a filter that asks for the same log twice and a
    // decoder allow-list that quietly disagrees with it.
    expect(new Set(SPOKE_INGESTED_TOPICS).size).toBe(13);
  });

  it('asks for exactly what it will decode', () => {
    // The two have to agree exactly: a topic requested but not decoded throws
    // on arrival, and a name decoded but never requested simply never comes.
    for (const name of SPOKE_INGESTED_EVENTS) expect(isIngestedSpokeEvent(name)).toBe(true);
    expect(SPOKE_INGESTED_TOPICS).toHaveLength(SPOKE_INGESTED_EVENTS.length);
  });

  it('keeps the fold’s inputs a named subset rather than the whole list', () => {
    // The distinction is load-bearing: the projections read position events,
    // and a config event reaching one of them would fold a risk parameter into
    // a share balance.
    for (const name of SPOKE_CONFIG_EVENTS) {
      expect(SPOKE_POSITION_EVENTS).not.toContain(name);
      expect(SPOKE_INGESTED_EVENTS).toContain(name);
    }
  });

  it('leaves out the events that describe a router rather than a position', () => {
    // §2: the position managers' mirror events are provenance records for
    // actions the Spoke has already reported, so folding them alongside would
    // double-count every routed action. They stay out permanently, not "for now".
    for (const name of ['SupplyOnBehalfOf', 'SetUserPositionManager', 'UpdatePositionManager']) {
      expect(isIngestedSpokeEvent(name)).toBe(false);
    }
  });

  it('leaves out the immutables event that carries the oracle', () => {
    // `SetSpokeImmutables` names the Spoke's oracle, and this deployment takes
    // that address from configuration instead — one log, once, against an
    // event plus a fold to read it. Pinned so the choice is visible rather than
    // an omission somebody re-derives later.
    expect(isIngestedSpokeEvent('SetSpokeImmutables')).toBe(false);
    expect(MAIN_SPOKE_ORACLE_ADDRESS).toBe('0x99b2b6cea9c3d2fd8f4d90f86741c44b212a6127');
  });

  it('defaults to a lower-cased address, since log addresses arrive that way', () => {
    expect(MAIN_SPOKE_ADDRESS).toBe('0x94e7a5dcbe816e498b89ab752661904e2f56c485');
  });
});

/** One event's parameters, widened so a spec can read names and flags off them. */
const inputs = (name: string) =>
  getAbiItem({ abi: SPOKE_ABI, name: name as 'AddReserve' }).inputs as readonly {
    name?: string;
    type: string;
    indexed?: boolean;
  }[];

describe('the config events’ shapes', () => {
  it('carries the collateral factor under a version, on both config events', () => {
    // §7.1 weights collateral by this, and §3 requires it keyed by version and
    // never overwritten in place. Both facts are in this signature.
    for (const name of ['AddDynamicReserveConfig', 'UpdateDynamicReserveConfig']) {
      expect(inputs(name).map((input) => input.name)).toEqual([
        'reserveId',
        'dynamicConfigKey',
        'config',
      ]);
      expect(inputs(name)[2]?.type).toBe('tuple');
    }
  });

  it('gives the refresh events no version at all', () => {
    // The difficulty the fold has to solve, stated by the ABI: a refresh says a
    // user moved to whatever the reserve's key was *at that block*, and does not
    // say which. Resolving it needs the config events' history.
    expect(inputs('RefreshAllUserDynamicConfig').map((input) => input.name)).toEqual(['user']);
    expect(inputs('RefreshSingleUserDynamicConfig').map((input) => input.name)).toEqual([
      'user',
      'reserveId',
    ]);
  });

  it('indexes the user first on a refresh, where every other event indexes a reserve', () => {
    // Why nothing may read a topic by position without knowing the event.
    // `topic1` is the reserve id on eleven of the thirteen and the user on two.
    expect(inputs('RefreshAllUserDynamicConfig')[0]).toMatchObject({
      name: 'user',
      indexed: true,
    });
    expect(inputs('AddReserve')[0]).toMatchObject({ name: 'reserveId', indexed: true });
  });
});
