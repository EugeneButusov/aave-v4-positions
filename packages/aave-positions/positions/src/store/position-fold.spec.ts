import { readFile } from 'node:fs/promises';
import { join } from 'node:path';

import { createClient, type ClickHouseClient } from '@clickhouse/client';
import {
  ClickHouseEventStore,
  EVENT_MIGRATIONS_DIR,
  type DecodedEvent,
} from '@aave-positions/events';
import { loadMigrations, migrate } from '@packages/clickhouse';
import type { Address } from '@packages/indexing';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import { ClickHousePositionStore, POSITION_MIGRATIONS_DIR } from './clickhouse-position-store';
import { encodeCursor } from './position-cursor';
import type { Position } from './position';

const CHAIN_ID = 1;
const SPOKE: Address = '0x94e7a5dcbe816e498b89ab752661904e2f56c485';
/** Checksummed, as viem hands them back. Lower-casing them is the fold's job. */
const ALICE = '0x82D16fF1C724ab72F218A3f7f6DD3E5385ee87E8';
const BOB = '0xB8516f75DCf450b5b455b5114F5a92F6abD37dCa';
/** A position manager, and never the owner of a position (§2). */
const ROUTER = '0xe68ab4F90Fe026B9873F5F276eD2d7efBbbE42Be';
/** Past 2^53 and past Int64 max, which is where JSONExtractInt returns 0. */
const HUGE = '422166581625087607993';

let client: ClickHouseClient;
let events: ClickHouseEventStore;
let store: ClickHousePositionStore;

interface At {
  readonly block: number;
  readonly log?: number;
}

function event(name: string, at: At, body: Record<string, unknown>): DecodedEvent {
  return {
    chainId: CHAIN_ID,
    address: SPOKE,
    blockNumber: at.block,
    blockHash: `0x${'aa'.repeat(32)}`,
    blockTimestamp: 1_785_000_000,
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

const NO_PREMIUM = { sharesDelta: '0', offsetRayDelta: '0', restoredPremiumRay: '0' };

const supply = (at: At, user: string, reserveId: string, shares: string, caller = user) =>
  event('Supply', at, {
    reserveId,
    caller,
    user,
    suppliedShares: shares,
    suppliedAmount: shares,
  });

const withdraw = (at: At, user: string, reserveId: string, shares: string) =>
  event('Withdraw', at, {
    reserveId,
    caller: user,
    user,
    withdrawnShares: shares,
    withdrawnAmount: shares,
  });

const borrow = (at: At, user: string, reserveId: string, shares: string) =>
  event('Borrow', at, { reserveId, caller: user, user, drawnShares: shares, drawnAmount: shares });

const repay = (at: At, user: string, reserveId: string, shares: string, premium = NO_PREMIUM) =>
  event('Repay', at, {
    reserveId,
    caller: user,
    user,
    drawnShares: shares,
    totalAmountRepaid: shares,
    premiumDelta: premium,
  });

const setCollateral = (at: At, user: string, reserveId: string, on: boolean) =>
  event('SetUsingAsCollateral', at, { reserveId, caller: user, user, usingAsCollateral: on });

const reportDeficit = (at: At, user: string, reserveId: string, shares: string) =>
  event('ReportDeficit', at, { reserveId, user, drawnShares: shares, premiumDelta: NO_PREMIUM });

const addReserve = (at: At, reserveId: string, assetId: string, hub: string) =>
  event('AddReserve', at, { reserveId, assetId, hub });

function liquidate(
  at: At,
  parts: {
    user: string;
    liquidator: string;
    collateralReserveId: string;
    debtReserveId: string;
    collateralSharesLiquidated: string;
    drawnSharesLiquidated: string;
    receiveShares?: boolean;
    collateralSharesToLiquidator?: string;
  },
): DecodedEvent {
  return event('LiquidationCall', at, {
    collateralReserveId: parts.collateralReserveId,
    debtReserveId: parts.debtReserveId,
    user: parts.user,
    liquidator: parts.liquidator,
    receiveShares: parts.receiveShares ?? false,
    debtAmountRestored: parts.drawnSharesLiquidated,
    drawnSharesLiquidated: parts.drawnSharesLiquidated,
    premiumDelta: NO_PREMIUM,
    collateralAmountRemoved: parts.collateralSharesLiquidated,
    collateralSharesLiquidated: parts.collateralSharesLiquidated,
    collateralSharesToLiquidator: parts.collateralSharesToLiquidator ?? '0',
  });
}

/** What the processor does with a dispatched range: cancel, then write. */
async function index(from: number, to: number, batch: DecodedEvent[]): Promise<void> {
  await events.revert(CHAIN_ID, from, to);
  await events.append(batch);
}

async function positions(user?: string): Promise<Position[]> {
  const page = await store.list({ chainId: CHAIN_ID, limit: 100, ...(user && { user }) });
  return [...page.items];
}

async function scalar(query: string): Promise<string> {
  const result = await client.query({ query, format: 'JSONEachRow' });
  const rows = await result.json<{ n: string | number }>();
  return String(rows[0]?.n);
}

/**
 * Writes a ledger row with a version this test chose.
 *
 * The only place that bypasses {@link ClickHouseEventStore}, and only because
 * the store stamps `Date.now()` — two dispatches inside one millisecond would
 * share a version and the ordering under test would not exist.
 */
async function ledgerRow(at: At, version: number, sign: 1 | -1, on: boolean): Promise<void> {
  await client.insert({
    table: 'spoke_events',
    format: 'JSONEachRow',
    values: [
      {
        chain_id: CHAIN_ID,
        address: SPOKE,
        block_number: at.block,
        log_index: at.log ?? 0,
        version,
        block_hash: `0x${'aa'.repeat(32)}`,
        block_timestamp: 1_785_000_000,
        tx_hash: `0x${'bb'.repeat(32)}`,
        tx_index: 0,
        event_name: 'SetUsingAsCollateral',
        topic1: null,
        topic2: null,
        topic3: null,
        body: JSON.stringify({ reserveId: '7', caller: ALICE, user: ALICE, usingAsCollateral: on }),
        data: '0x',
        sign,
      },
    ],
  });
}

const CONNECTION = {
  url: process.env['CLICKHOUSE_URL'] ?? 'http://localhost:8123',
  username: process.env['CLICKHOUSE_USER'] ?? 'default',
  password: process.env['CLICKHOUSE_PASSWORD'] ?? '',
};

/**
 * A database of this suite's own, and not a tidiness preference.
 *
 * The events package's specs run against the same server and the same table
 * names. Sharing one database has them truncating each other's fixtures, and —
 * worse — the projections created here start firing on that suite's synthetic
 * bodies, which are minimal by design and carry no `reserveId`. Its specs would
 * fail for a reason that has nothing to do with what they test.
 */
const DATABASE = 'spec_positions';

describe('the position fold', () => {
  beforeAll(async () => {
    const bootstrap = createClient(CONNECTION);
    await bootstrap.command({ query: `CREATE DATABASE IF NOT EXISTS ${DATABASE}` });
    await bootstrap.close();

    client = createClient({ ...CONNECTION, database: DATABASE });
    // Both directories, ordered by ordinal across them — which is what puts the
    // views after the table they read.
    await migrate(client, await loadMigrations([EVENT_MIGRATIONS_DIR, POSITION_MIGRATIONS_DIR]));
    events = new ClickHouseEventStore(client);
    store = new ClickHousePositionStore(client);
  });

  afterAll(async () => {
    await client.close();
  });

  beforeEach(async () => {
    // Test-only. Nothing in either package removes a row.
    for (const table of ['spoke_events', 'user_positions', 'user_position_flags', 'spoke_reserves'])
      // oxlint-disable-next-line no-await-in-loop
      await client.command({ query: `TRUNCATE TABLE ${table}` });
  });

  it('keys the position on the user, lower-cased, and never on the caller', async () => {
    await index(100, 100, [supply({ block: 100 }, ALICE, '7', '500', ROUTER)]);

    const [position] = await positions();
    // §2: routing through a position manager is how a book gets attributed to
    // three addresses. And the body carries checksummed addresses, so a caller
    // passing the lower-case form they read off a URL has to still match.
    expect(position?.user).toBe(ALICE.toLowerCase());
    expect(position?.suppliedShares).toBe('500');
  });

  it('keeps a share balance past 2^53 exact', async () => {
    await index(100, 100, [supply({ block: 100 }, ALICE, '7', HUGE)]);

    // JSONExtractInt would make this 0 — not a truncation, a zero, and only
    // above Int64 max. It works for small positions and silently empties large
    // ones, which is why the projections parse through toInt256 (§7.5).
    expect((await positions())[0]?.suppliedShares).toBe(HUGE);
  });

  it('nets a withdrawal against the supply', async () => {
    await index(100, 200, [
      supply({ block: 100 }, ALICE, '7', '500'),
      withdraw({ block: 200 }, ALICE, '7', '200'),
    ]);

    expect((await positions())[0]).toMatchObject({ suppliedShares: '300', events: 2 });
  });

  it('folds borrow and repay onto the debt side, premium included', async () => {
    await index(100, 200, [
      borrow({ block: 100 }, ALICE, '7', '500'),
      repay({ block: 200 }, ALICE, '7', '200', {
        sharesDelta: '-30',
        offsetRayDelta: '7',
        restoredPremiumRay: '0',
      }),
    ]);

    // The premium triple has never been non-zero on mainnet (§5.4), so this is
    // the only thing that will ever check the sign convention.
    expect((await positions())[0]).toMatchObject({
      drawnShares: '300',
      premiumShares: '-30',
      premiumOffsetRay: '7',
    });
  });

  it('leaves the position unchanged when the identical range is dispatched again', async () => {
    const batch = [supply({ block: 100 }, ALICE, '7', '500')];

    await index(100, 200, batch);
    await index(100, 200, batch);

    // Dispatch is at-least-once: a processor that returns ok is re-invoked
    // whenever a later one asks to retry. Nothing in this package coordinates
    // with that — the ledger's retraction reaches the projection through the
    // `sign *` in every view, and the sums cancel.
    expect((await positions())[0]).toMatchObject({ suppliedShares: '500', events: 1 });
  });

  it('empties the position when a reorg retracts its range', async () => {
    await index(100, 200, [supply({ block: 100 }, ALICE, '7', '500')]);

    await events.revert(CHAIN_ID, 100, 200);

    expect(await positions()).toEqual([]);
  });

  it('re-indexes the new branch after a reorg', async () => {
    await index(100, 200, [supply({ block: 100 }, ALICE, '7', '500')]);
    await events.revert(CHAIN_ID, 100, 200);

    await index(100, 200, [supply({ block: 100 }, ALICE, '7', '60')]);

    expect((await positions())[0]?.suppliedShares).toBe('60');
  });

  it('leaves blocks outside the retracted range alone', async () => {
    await index(100, 100, [supply({ block: 100 }, ALICE, '7', '500')]);
    await index(200, 200, [supply({ block: 200 }, ALICE, '7', '40')]);

    await events.revert(CHAIN_ID, 200, 200);

    expect((await positions())[0]?.suppliedShares).toBe('500');
  });

  describe('the collateral flag', () => {
    it('takes the latest by chain order', async () => {
      await index(100, 200, [
        setCollateral({ block: 100 }, ALICE, '7', true),
        setCollateral({ block: 200 }, ALICE, '7', false),
        supply({ block: 100, log: 1 }, ALICE, '7', '500'),
      ]);

      expect((await positions())[0]?.usingAsCollateral).toBe(false);
    });

    it('returns the earlier flag once the later one is retracted', async () => {
      await index(100, 100, [
        setCollateral({ block: 100 }, ALICE, '7', true),
        supply({ block: 100, log: 1 }, ALICE, '7', '500'),
      ]);
      await index(200, 200, [setCollateral({ block: 200 }, ALICE, '7', false)]);

      await events.revert(CHAIN_ID, 200, 200);

      // The reason the flag cannot be pre-aggregated at all. argMaxState has no
      // inverse, a ReplacingMergeTree tombstone would delete the key rather than
      // the generation, and argMaxIf(..., sign = 1) keeps the retracted event
      // because its +1 twin still carries sign = 1 — measured, all three.
      // Liveness is a property of the (key, version) group, so the collapse in
      // user_positions_current is what makes this work.
      expect((await positions())[0]?.usingAsCollateral).toBe(true);
    });

    it('is not moved by a range re-dispatched out of block order', async () => {
      await ledgerRow({ block: 100 }, 1000, 1, true);
      await ledgerRow({ block: 200 }, 2000, 1, false);
      // A later processor asked to retry, so block 100 goes round again and gets
      // the higher version — while block 200, the later chain event, keeps the
      // lower one.
      await ledgerRow({ block: 100 }, 1000, -1, true);
      await ledgerRow({ block: 100 }, 5000, 1, true);
      await index(300, 300, [supply({ block: 300 }, ALICE, '7', '500')]);

      // Ordering the argMax by `version` reads `true` here. `version` orders
      // dispatches; (block_number, log_index) orders chain events, and only the
      // second is what "the current flag" means.
      expect((await positions())[0]?.usingAsCollateral).toBe(false);
    });
  });

  describe('LiquidationCall', () => {
    it('applies the collateral and debt legs to the borrower', async () => {
      await index(100, 200, [
        supply({ block: 100 }, ALICE, '7', '500'),
        borrow({ block: 100, log: 1 }, ALICE, '13', '400'),
        liquidate(
          { block: 200 },
          {
            user: ALICE,
            liquidator: BOB,
            collateralReserveId: '7',
            debtReserveId: '13',
            collateralSharesLiquidated: '150',
            drawnSharesLiquidated: '100',
          },
        ),
      ]);

      const held = await positions(ALICE);
      expect(held.map((p) => [p.reserveId, p.suppliedShares, p.drawnShares])).toEqual([
        ['7', '350', '0'],
        ['13', '0', '300'],
      ]);
      // receiveShares was false, so the liquidator gained nothing.
      expect(await positions(BOB)).toEqual([]);
    });

    it('adds both borrower legs to one row when the liquidation is self-collateralised', async () => {
      await index(100, 200, [
        supply({ block: 100 }, ALICE, '7', '500'),
        borrow({ block: 100, log: 1 }, ALICE, '7', '400'),
        liquidate(
          { block: 200 },
          {
            user: ALICE,
            liquidator: BOB,
            collateralReserveId: '7',
            debtReserveId: '7',
            collateralSharesLiquidated: '150',
            drawnSharesLiquidated: '100',
          },
        ),
      ]);

      // Both legs land on the same sorting key. SummingMergeTree adds them into
      // one row carrying both deltas, which is why no leg discriminator column
      // exists — under a collapsing engine two +1 rows at one key would not have
      // been safe.
      expect(await positions(ALICE)).toEqual([
        expect.objectContaining({ reserveId: '7', suppliedShares: '350', drawnShares: '300' }),
      ]);
    });

    it('credits the liquidator when receiveShares is set, with no Supply anywhere', async () => {
      const batch = [
        liquidate(
          { block: 200 },
          {
            user: ALICE,
            liquidator: BOB,
            collateralReserveId: '7',
            debtReserveId: '13',
            collateralSharesLiquidated: '150',
            drawnSharesLiquidated: '100',
            receiveShares: true,
            collateralSharesToLiquidator: '140',
          },
        ),
      ];
      await index(200, 200, batch);

      // §4.1's trap 3, and 0 of 90 mainnet liquidations have taken this branch —
      // so this spec is the only proof the path will ever have. Crediting
      // supplied shares only on Supply would silently under-count every
      // liquidator, with nothing in the logs to show for it. The 10-share gap to
      // collateralSharesLiquidated is the protocol fee, which settles on the Hub
      // and touches no user position.
      expect(await positions(BOB)).toEqual([
        expect.objectContaining({ reserveId: '7', suppliedShares: '140' }),
      ]);
      expect(batch[0]?.body).not.toHaveProperty('suppliedShares');
    });
  });

  it('writes off drawn shares on ReportDeficit', async () => {
    await index(100, 200, [
      borrow({ block: 100 }, ALICE, '13', '400'),
      reportDeficit({ block: 200 }, ALICE, '13', '400'),
    ]);

    // §4.1's trap 2: bad debt is a separate event, and folding only
    // LiquidationCall leaves the borrower carrying phantom debt forever.
    expect(await positions(ALICE)).toEqual([]);
  });

  describe('when a projection misses a row', () => {
    /** A Supply with no `suppliedShares`, which every projection of it must reject. */
    const malformed = event('Supply', { block: 500 }, { reserveId: '7', user: ALICE });

    it('fails the insert rather than writing a zero', async () => {
      await expect(events.append([malformed])).rejects.toThrow(/suppliedShares|Int256/);
    });

    it('commits the ledger row anyway, and then cannot move past it', async () => {
      await events.append([malformed]).catch(() => undefined);

      // The ledger row lands even though the insert reported failure. The
      // projection never saw it.
      expect(await scalar(`SELECT count() AS n FROM spoke_events`)).toBe('1');
      expect(await scalar(`SELECT count() AS n FROM user_positions`)).toBe('0');

      // And the range is now stuck: the revert copies the same body back for the
      // projection to choke on again, so every retry fails identically. Loud and
      // halted, which beats quietly wrong — but it is a halt, not a self-repair.
      await expect(events.revert(CHAIN_ID, 500, 500)).rejects.toThrow(/suppliedShares|Int256/);
    });

    it('cannot be repaired by re-dispatching the range', async () => {
      // A projection that missed rows, arranged the way it actually happens: the
      // view did not exist when they were written. Identical to adding a
      // projection to a populated ledger.
      await client.command({ query: `DROP VIEW position_supply` });
      await index(500, 500, [supply({ block: 500 }, ALICE, '7', '100')]);
      await client.command({
        query: await readFile(join(POSITION_MIGRATIONS_DIR, '013_position_supply.sql'), 'utf8'),
      });

      await index(500, 500, [supply({ block: 500 }, ALICE, '7', '100')]);

      // Short by exactly the missed event: the revert subtracted a contribution
      // the projection never received. Recovery is truncate-and-replay — the
      // same remedy the README gives for a projection added late — and neither a
      // retry nor a merge substitutes for it.
      expect(await positions()).toEqual([]);
      expect(await scalar(`SELECT toString(sum(supplied_shares)) AS n FROM user_positions`)).toBe(
        '0',
      );
    });
  });

  describe('the store', () => {
    it('hides a closed position but keeps its history', async () => {
      await index(100, 200, [
        supply({ block: 100 }, ALICE, '7', '500'),
        withdraw({ block: 200 }, ALICE, '7', '500'),
      ]);

      expect(await positions()).toEqual([]);
      // The row is still there — §12.1 says a position *exists* while its shares
      // are non-zero, not that its history stops having happened.
      expect(await scalar(`SELECT sum(events) AS n FROM user_positions`)).toBe('2');
    });

    it('resolves the asset from the registry, and reads null before AddReserve lands', async () => {
      await index(100, 100, [supply({ block: 100 }, ALICE, '7', '500')]);
      expect((await positions())[0]).toMatchObject({ assetId: null, hub: null });

      await index(200, 200, [
        addReserve({ block: 200 }, '7', '3', '0xCca852Bc40e560adC3b1Cc58CA5b55638ce826c9'),
      ]);

      // Resolved at read time, never joined at insert time: a materialized view
      // sees only the block being inserted, and the backfill's first chunk can
      // carry a Supply and the AddReserve that names its asset together.
      expect((await positions())[0]).toMatchObject({
        assetId: '3',
        hub: '0xcca852bc40e560adc3b1cc58ca5b55638ce826c9',
      });
    });

    it('pages through every position exactly once', async () => {
      const seeded = ['3', '7', '13', '21', '34'];
      await index(
        100,
        100,
        seeded.map((reserveId, i) => supply({ block: 100, log: i }, ALICE, reserveId, '500')),
      );

      const seen: string[] = [];
      let cursor: string | undefined;
      do {
        // Sequential by definition — each page's cursor comes from the last one.
        // oxlint-disable-next-line no-await-in-loop
        const page = await store.list({ chainId: CHAIN_ID, limit: 2, ...(cursor && { cursor }) });
        seen.push(...page.items.map((p) => p.reserveId));
        cursor = page.nextCursor ?? undefined;
      } while (cursor !== undefined);

      // Keyset, so the boundary is a key rather than a row count: nothing is
      // returned twice or skipped even though the indexer is free to write
      // between pages. Sorted as UInt256, so 13 precedes 21 rather than 3.
      expect(seen).toEqual(['3', '7', '13', '21', '34']);
    });

    it('reports no next cursor on the last page', async () => {
      await index(100, 100, [supply({ block: 100 }, ALICE, '7', '500')]);

      expect(await store.list({ chainId: CHAIN_ID, limit: 1 })).toMatchObject({ nextCursor: null });
    });

    it('accepts a checksummed address and a cursor it issued', async () => {
      await index(100, 100, [
        supply({ block: 100, log: 0 }, ALICE, '7', '500'),
        supply({ block: 100, log: 1 }, ALICE, '13', '500'),
      ]);

      const page = await store.list({
        chainId: CHAIN_ID,
        user: ALICE,
        limit: 10,
        cursor: encodeCursor({ user: ALICE.toLowerCase(), spoke: SPOKE, reserveId: '7' }),
      });

      expect(page.items.map((p) => p.reserveId)).toEqual(['13']);
    });
  });

  it('issues no DELETE and no ALTER against anything it maintains', async () => {
    await index(100, 200, [supply({ block: 100 }, ALICE, '7', '500')]);
    await events.revert(CHAIN_ID, 100, 200);

    // The projection inherits the ledger's discipline: a reorg is an append, and
    // if that ever stops holding, the immutability the whole design rests on has
    // quietly stopped being real.
    expect(
      await scalar(
        `SELECT count() AS n FROM system.mutations
         WHERE table IN ('user_positions', 'user_position_flags', 'spoke_reserves')`,
      ),
    ).toBe('0');
  });
});
