import type { ClickHouseClient } from '@clickhouse/client';
import {
  ClickHouseHubEventStore,
  ClickHouseSpokeEventStore,
  type DecodedEvent,
} from '@aave-positions/events';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import { ClickHousePositionStore } from './clickhouse-position-store';
import { PositionCursorCodec } from './position-cursor';
import { RAY } from '../valuation/valuation';
import {
  HUB,
  HUB_TABLES,
  USDC,
  add,
  addAsset,
  draw,
  updateAsset,
} from '../test-support/hub-ledger';
import {
  ALICE,
  CHAIN_ID,
  SPOKE,
  TABLES,
  addReserve,
  borrow,
  migratedDatabase,
  supply,
} from '../test-support/spoke-ledger';

/** Its own database: sibling suites share table names and would truncate this one. */
const DATABASE = 'spec_position_valuation';

/** The checkpoint every fixture below extrapolates from. */
const CHECKPOINT_BLOCK = 100;
const CHECKPOINT_AT = 1_785_000_000 + CHECKPOINT_BLOCK;
const YEAR = 365 * 24 * 3600;
/** 5% per annum, RAY-scaled, as `drawnRate` arrives on `UpdateAsset`. */
const FIVE_PERCENT = (RAY / 20n).toString();

let client: ClickHouseClient;
let spokeEvents: ClickHouseSpokeEventStore;
let hubEvents: ClickHouseHubEventStore;
let store: ClickHousePositionStore;

const page = (over: Partial<Parameters<ClickHousePositionStore['list']>[0]> = {}) =>
  store.list({ chainId: CHAIN_ID, user: ALICE, spoke: SPOKE, limit: 100, ...over });

/**
 * A reserve that resolves all the way to a token, and a Hub asset with a
 * checkpoint — the state valuation needs before it can produce a number.
 *
 * Asset 7 borrows 400,000 of the 1,000,000 supplied, so the index actually
 * accrues: the short-circuit would hold it at RAY if nothing were drawn.
 */
async function listReserve(events: DecodedEvent[] = []): Promise<void> {
  await hubEvents.append([
    addAsset({ block: 10 }, USDC, 6),
    add({ block: 20 }, '1000000', '1000000'),
    draw({ block: 30 }, '400000', '400000'),
    updateAsset({ block: CHECKPOINT_BLOCK }, RAY.toString(), FIVE_PERCENT, '0'),
  ]);
  await spokeEvents.append([addReserve({ block: 10 }, '7', '7', HUB), ...events]);
}

describe('valuing a position', () => {
  beforeAll(async () => {
    client = await migratedDatabase(DATABASE);
    spokeEvents = new ClickHouseSpokeEventStore(client);
    hubEvents = new ClickHouseHubEventStore(client);
    store = new ClickHousePositionStore(
      client,
      new PositionCursorCodec('spec-cursor-secret'.padEnd(32, '.')),
    );
  });

  afterAll(async () => {
    await client.close();
  });

  beforeEach(async () => {
    for (const table of [...TABLES, ...HUB_TABLES])
      // oxlint-disable-next-line no-await-in-loop
      await client.command({ query: `TRUNCATE TABLE ${table}` });
  });

  describe('the registry', () => {
    it('resolves a reserve to its Hub asset and token', async () => {
      await listReserve([supply({ block: 200 }, ALICE, '7', '1000')]);

      // reserveId is a per-Spoke index and means nothing on its own (§1).
      // AddReserve gives it a Hub and an assetId; the Hub's AddAsset gives that
      // an ERC-20 and its decimals. Neither contract has both halves.
      expect((await page()).items[0]?.asset).toEqual({
        assetId: '7',
        hub: HUB,
        underlying: USDC,
        decimals: 6,
      });
    });

    it('reports null rather than zero for a reserve it has never seen', async () => {
      await spokeEvents.append([supply({ block: 200 }, ALICE, '99', '1000')]);

      // A zero here is indistinguishable from a real zero balance. The position
      // still appears, because its shares are real.
      const [position] = (await page()).items;
      expect(position?.suppliedShares).toBe('1000');
      expect(position?.asset).toBeNull();
      expect(position?.value).toBeNull();
    });

    it('reports null when the Hub has listed the asset but never checkpointed it', async () => {
      await hubEvents.append([addAsset({ block: 10 }, USDC, 6)]);
      await spokeEvents.append([
        addReserve({ block: 10 }, '7', '7', HUB),
        supply({ block: 200 }, ALICE, '7', '1000'),
      ]);

      // No UpdateAsset means no index, and without an index there is no
      // arithmetic to do — so no number is offered.
      expect((await page()).items[0]?.value).toBeNull();
    });
  });

  describe('the amounts', () => {
    it('turns supplied shares into a token amount', async () => {
      await listReserve([supply({ block: 200 }, ALICE, '7', '1000')]);

      const [position] = (await page({ asOf: BigInt(CHECKPOINT_AT) })).items;
      // Valued at the checkpoint itself, so the index has not moved: the asset
      // holds 1,000,000 shares against 1,000,000 of underlying, and 1,000 shares
      // redeem for 1,000.
      expect(position?.value?.suppliedAmount).toBe('1000');
      expect(position?.value?.drawnIndex).toBe(RAY.toString());
    });

    it('grows a debt with time on a fixed share balance', async () => {
      await listReserve([borrow({ block: 200 }, ALICE, '7', '1000000')]);

      const now = await page({ asOf: BigInt(CHECKPOINT_AT) });
      const later = await page({ asOf: BigInt(CHECKPOINT_AT + YEAR) });

      // The whole reason a share balance is not a balance (§5): nothing was
      // indexed between these two reads.
      expect(now.items[0]?.value?.totalDebt).toBe('1000000');
      expect(later.items[0]?.value?.totalDebt).toBe('1050000');
      expect(now.items[0]?.drawnShares).toBe(later.items[0]?.drawnShares);
    });

    it('keeps the shares and the flow beside the amount', async () => {
      await listReserve([supply({ block: 200 }, ALICE, '7', '1000')]);

      // Cost basis and current value answer different questions, and the
      // difference between them is interest — so neither replaces the other.
      expect((await page()).items[0]).toMatchObject({
        suppliedShares: '1000',
        netSuppliedAmount: '1000',
        value: expect.objectContaining({ suppliedAmount: expect.any(String) }),
      });
    });

    it('values every position on a page at one instant', async () => {
      await listReserve([
        supply({ block: 200, log: 0 }, ALICE, '7', '1000'),
        borrow({ block: 200, log: 1 }, ALICE, '7', '500'),
      ]);

      const result = await page({ asOf: BigInt(CHECKPOINT_AT + YEAR) });

      // One instant for the whole page, reported back: an amount without the
      // moment it was computed at is not reproducible (§12.6).
      expect(result.valuedAt).toBe(CHECKPOINT_AT + YEAR);
      const indexes = new Set(result.items.map((p) => p.value?.drawnIndex));
      expect(indexes.size).toBe(1);
    });

    it('defaults to now when no instant is named', async () => {
      await listReserve([supply({ block: 200 }, ALICE, '7', '1000')]);

      const before = Math.floor(Date.now() / 1000);
      const result = await page();

      // Which is what the chain does — getUserDebt at `latest` extrapolates to
      // the head block rather than to the last event.
      expect(result.valuedAt).toBeGreaterThanOrEqual(before);
      expect(result.items[0]?.value?.drawnIndex).not.toBe(RAY.toString());
    });

    it('returns every amount as a string, exact past 2^53', async () => {
      await listReserve([borrow({ block: 200 }, ALICE, '7', '422166581625087607993')]);

      const [position] = (await page({ asOf: BigInt(CHECKPOINT_AT) })).items;
      expect(position?.value?.drawnDebt).toBe('422166581625087607993');
      expect(typeof position?.value?.totalDebt).toBe('string');
    });
  });
});
