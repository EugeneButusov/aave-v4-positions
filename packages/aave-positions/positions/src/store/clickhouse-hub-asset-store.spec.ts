import type { ClickHouseClient } from '@clickhouse/client';
import { ClickHouseHubEventStore, type DecodedEvent } from '@aave-positions/events';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import { ClickHouseHubAssetStore } from './clickhouse-hub-asset-store';
import {
  HUB,
  HUB_TABLES,
  RAY,
  USDC,
  add,
  addAsset,
  draw,
  refreshPremium,
  reportDeficit,
  sweep,
  updateAsset,
  updateAssetConfig,
} from '../test-support/hub-ledger';
import { CHAIN_ID, migratedDatabase } from '../test-support/spoke-ledger';

/** Its own database: sibling suites share table names and would truncate this one. */
const DATABASE = 'spec_hub_asset_store';
/** Plus Hub on mainnet. A second Hub lists its own asset 7. */
const OTHER_HUB = '0x06002e9c4412cb7814a791ea3666d905871e536a';
const OTHER_CHAIN = 8453;

let client: ClickHouseClient;
let events: ClickHouseHubEventStore;
let store: ClickHouseHubAssetStore;

/** The same event, re-addressed — what a second Hub or a second chain looks like. */
const elsewhere = (
  event: DecodedEvent,
  over: { address?: string; chainId?: number },
): DecodedEvent => ({ ...event, ...over });

/**
 * One asset with a **distinct value in every column**, so a mapper that crossed
 * two fields cannot pass. Everything here is chosen to be unlike its neighbours
 * rather than to be realistic.
 *
 * liquidity 100 − 10 drawn − 4 swept = 86.
 */
const DISTINCT = [
  add({ block: 100, log: 0 }, '2', '100'),
  draw({ block: 100, log: 1 }, '3', '10'),
  sweep({ block: 100, log: 2 }, '4'),
  refreshPremium({ block: 100, log: 3 }, '5', '-6'),
  reportDeficit({ block: 100, log: 4 }, '0', '7'),
  addAsset({ block: 100, log: 5 }, USDC, 8),
  updateAssetConfig({ block: 100, log: 6 }, 9),
  updateAsset({ block: 100, log: 7 }, RAY, '11', '12'),
];

describe('ClickHouseHubAssetStore', () => {
  beforeAll(async () => {
    client = await migratedDatabase(DATABASE);
    events = new ClickHouseHubEventStore(client);
    store = new ClickHouseHubAssetStore(client);
  });

  afterAll(async () => {
    await client.close();
  });

  beforeEach(async () => {
    for (const table of HUB_TABLES)
      // oxlint-disable-next-line no-await-in-loop
      await client.command({ query: `TRUNCATE TABLE ${table}` });
  });

  describe('scope', () => {
    it('returns one Hub, and not another holding the same asset id', async () => {
      await events.append([
        add({ block: 100 }, '1000', '1000', '7'),
        elsewhere(add({ block: 100, log: 1 }, '9999', '9999', '7'), { address: OTHER_HUB }),
      ]);

      // Asset ids are Hub-scoped, so both of these are "asset 7" and only one
      // of them is this Hub's.
      expect((await store.list(CHAIN_ID, HUB)).map((a) => a.addedShares)).toEqual(['1000']);
      expect((await store.get(CHAIN_ID, HUB, '7'))?.addedShares).toBe('1000');
    });

    it('returns one chain, and not another running the same Hub address', async () => {
      await events.append([
        add({ block: 100 }, '1000', '1000', '7'),
        elsewhere(add({ block: 100, log: 1 }, '9999', '9999', '7'), { chainId: OTHER_CHAIN }),
      ]);

      // The same contract address is deployed on more than one chain, so
      // `chain_id` is part of the key rather than a convenience.
      expect((await store.list(CHAIN_ID, HUB)).map((a) => a.addedShares)).toEqual(['1000']);
      expect((await store.get(OTHER_CHAIN, HUB, '7'))?.addedShares).toBe('9999');
    });

    it('lower-cases the address it is given', async () => {
      await events.append([add({ block: 100 }, '1000', '1000', '7')]);

      // A caller reads a checksummed address off a block explorer or the address
      // book — `AaveV4Ethereum.HUBS.CORE_HUB` is checksummed — while the mirror
      // stores what the log carried, which is lower-case.
      const checksummed = '0xCca852Bc40e560adC3b1Cc58CA5b55638ce826c9';
      expect(await store.list(CHAIN_ID, checksummed)).toHaveLength(1);
      expect(await store.get(CHAIN_ID, checksummed, '7')).not.toBeNull();
    });

    it('finds nothing on a Hub it has never seen', async () => {
      await events.append([add({ block: 100 }, '1000', '1000', '7')]);

      expect(await store.list(CHAIN_ID, OTHER_HUB)).toEqual([]);
      expect(await store.get(CHAIN_ID, OTHER_HUB, '7')).toBeNull();
    });

    it('finds nothing for an asset the Hub has never listed', async () => {
      await events.append([add({ block: 100 }, '1000', '1000', '7')]);

      expect(await store.get(CHAIN_ID, HUB, '99')).toBeNull();
    });
  });

  describe('ordering', () => {
    it('lists assets in numeric order, not text order', async () => {
      await events.append([
        add({ block: 100, log: 0 }, '1', '1', '3'),
        add({ block: 100, log: 1 }, '1', '1', '13'),
        add({ block: 100, log: 2 }, '1', '1', '21'),
      ]);

      // `ORDER BY a.asset_id` has to be qualified: unqualified it binds the
      // `toString(asset_id)` alias in the projection and sorts the digits as
      // text, giving 13, 21, 3 — the same defect the position store's
      // pagination hit, where it handed the next page a cursor from the wrong
      // row.
      expect((await store.list(CHAIN_ID, HUB)).map((a) => a.assetId)).toEqual(['3', '13', '21']);
    });
  });

  describe('the row it returns', () => {
    it('maps every column to its own field', async () => {
      await events.append(DISTINCT);

      // Every value distinct, so crossing two fields in the mapper fails here
      // rather than surfacing as a wrong balance much later. This is the whole
      // reason the test exists: the mirror's own specs read these back through
      // the same mapper, so they cannot see a mapping that is internally
      // consistent and wrong.
      expect(await store.get(CHAIN_ID, HUB, '7')).toEqual({
        chainId: CHAIN_ID,
        hub: HUB,
        assetId: '7',
        liquidity: '86',
        addedShares: '2',
        drawnShares: '3',
        swept: '4',
        premiumShares: '5',
        premiumOffsetRay: '-6',
        deficitRay: '7',
        underlying: USDC,
        decimals: 8,
        liquidityFee: 9,
        drawnIndex: RAY,
        drawnRate: '11',
        realizedFees: '12',
        indexTimestamp: '2026-07-25T17:21:40Z',
        events: 5,
      });
    });

    it('returns wide integers as strings and the rest as themselves', async () => {
      await events.append([
        updateAsset(
          { block: 100 },
          '115792089237316195423570985008687907853269984665640564039457584007913129639935',
        ),
        add({ block: 101 }, '1', '1'),
      ]);

      const asset = await store.get(CHAIN_ID, HUB, '7');
      // uint256 max. A JSON number would have lost its tail before it reached
      // this process (§7.5).
      expect(asset?.drawnIndex).toBe(
        '115792089237316195423570985008687907853269984665640564039457584007913129639935',
      );
      expect(typeof asset?.assetId).toBe('string');
      expect(typeof asset?.liquidity).toBe('string');
      // Small and bounded, so they stay numbers: a fee is basis points and a
      // count is a count.
      expect(typeof asset?.events).toBe('number');
      expect(typeof asset?.chainId).toBe('number');
    });

    it('reports the checkpoint as an unambiguous UTC instant', async () => {
      await events.append([add({ block: 100 }, '1', '1'), updateAsset({ block: 200 })]);

      // Formatted in SQL rather than left to the driver, so the caller is not
      // reading a local-time rendering and guessing. The valuation turns it
      // straight back into seconds.
      const at = (await store.get(CHAIN_ID, HUB, '7'))?.indexTimestamp;
      expect(at).toBe('2026-07-25T17:23:20Z');
      expect(Date.parse(at ?? '') / 1000).toBe(1_785_000_200);
    });

    it('leaves the latest-wins fields null until their event arrives', async () => {
      await events.append([add({ block: 100 }, '1000', '1000', '7')]);

      // A listing with no checkpoint yet is a real state — the valuation reports
      // no amount rather than inventing a zero index, and that decision needs
      // null to survive the mapper.
      expect(await store.get(CHAIN_ID, HUB, '7')).toMatchObject({
        drawnIndex: null,
        drawnRate: null,
        realizedFees: null,
        indexTimestamp: null,
        liquidityFee: null,
        underlying: null,
        decimals: null,
      });
    });

    it('keeps an asset whose balances net to zero', async () => {
      await events.append([
        addAsset({ block: 100 }, USDC, 6),
        add({ block: 101 }, '1000', '1000'),
        elsewhere(add({ block: 102 }, '1000', '1000'), {}),
      ]);
      await events.revert(CHAIN_ID, 101, 102);

      // Unlike a position, an emptied asset is still a listing with a token
      // address. Dropping it would leave a position against it unvaluable
      // rather than valued at zero — so there is no `!= 0` filter here.
      expect(await store.get(CHAIN_ID, HUB, '7')).toMatchObject({
        addedShares: '0',
        underlying: USDC,
        events: 0,
      });
    });
  });
});
