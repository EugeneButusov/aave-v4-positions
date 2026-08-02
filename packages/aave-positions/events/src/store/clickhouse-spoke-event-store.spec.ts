import { createClient, type ClickHouseClient } from '@clickhouse/client';
import type { Address } from '@packages/indexing';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import type { DecodedEvent } from '../decode/decoded-event';
import { EVENT_MIGRATIONS_DIR } from './clickhouse-event-store';
import {
  ClickHouseSpokeEventStore,
  SPOKE_EVENTS_TABLE as EVENTS_TABLE,
  SPOKE_EVENTS_VIEW as EVENTS_VIEW,
} from './clickhouse-spoke-event-store';
import { migrate } from '@packages/clickhouse';
import { loadMigrations } from '@packages/migrations';

/** Its own database, for the reason spelled out in `beforeAll`. */
const DATABASE = 'spec_spoke_events';
const CONNECTION = {
  url: process.env['CLICKHOUSE_URL'] ?? 'http://localhost:8123',
  username: process.env['CLICKHOUSE_USER'] ?? 'default',
  password: process.env['CLICKHOUSE_PASSWORD'] ?? '',
};

const CHAIN_ID = 1;
const SPOKE: Address = '0x94e7a5dcbe816e498b89ab752661904e2f56c485';
/** Well past 2^53, which is the whole point of the UInt256 columns (§7.5). */
const HUGE_SHARES =
  '115792089237316195423570985008687907853269984665640564039457584007913129639935';

let client: ClickHouseClient;
let store: ClickHouseSpokeEventStore;

function event(overrides: Partial<DecodedEvent> = {}): DecodedEvent {
  return {
    chainId: CHAIN_ID,
    address: SPOKE,
    blockNumber: 1000,
    blockHash: `0x${'aa'.repeat(32)}`,
    blockTimestamp: 1_785_000_000,
    txHash: `0x${'bb'.repeat(32)}`,
    txIndex: 0,
    logIndex: 0,
    eventName: 'Supply',
    topic1: `0x${'07'.padStart(64, '0')}`,
    topic2: `0x${'22'.repeat(20).padStart(64, '0')}`,
    topic3: `0x${'11'.repeat(20).padStart(64, '0')}`,
    body: { suppliedShares: HUGE_SHARES },
    data: '0x00',
    ...overrides,
  };
}

async function live(): Promise<{ block_number: number; log_index: number; body: string }[]> {
  const result = await client.query({
    query: `SELECT block_number, log_index, body FROM ${EVENTS_VIEW}
            WHERE chain_id = ${CHAIN_ID} ORDER BY block_number, log_index`,
    format: 'JSONEachRow',
  });
  return result.json();
}

async function liveCount(): Promise<number> {
  return (await live()).length;
}

/** ClickHouse renders a UInt64 as a string or a number depending on context. */
async function scalar(query: string): Promise<number> {
  const result = await client.query({ query, format: 'JSONEachRow' });
  const rows = await result.json<{ n: string | number }>();
  return Number(rows[0]?.n);
}

describe('ClickHouseSpokeEventStore', () => {
  beforeAll(async () => {
    const bootstrap = createClient(CONNECTION);
    await bootstrap.command({ query: `CREATE DATABASE IF NOT EXISTS ${DATABASE}` });
    await bootstrap.close();

    client = createClient({ ...CONNECTION, database: DATABASE });
    // This package's migrations only. The bodies below are minimal by design —
    // most carry no `reserveId` — so the positions projections would throw on
    // them and fail eleven specs for a reason unrelated to what they test.
    // Running the application's `migrate` against a local ClickHouse is enough
    // to put those projections in `default`, which is why this suite does not
    // use it.
    await migrate(client, await loadMigrations([EVENT_MIGRATIONS_DIR]));
    store = new ClickHouseSpokeEventStore(client);
  });

  afterAll(async () => {
    await client.close();
  });

  beforeEach(async () => {
    // TRUNCATE resets the fixture between tests. It is a test-only convenience;
    // nothing in the store itself removes a row.
    await client.command({ query: `TRUNCATE TABLE ${EVENTS_TABLE}` });
  });

  /**
   * What the processor does on a dispatched range: cancel whatever generation
   * is stored for it, then write the new one. Two calls, and the order is the
   * contract — the spec below proves what happens without the first.
   */
  async function index(from: number, to: number, events: DecodedEvent[]): Promise<void> {
    await store.revert(CHAIN_ID, from, to);
    await store.append(events);
  }

  it('stores a uint256 above 2^53 without losing a digit', async () => {
    await index(1000, 1000, [event()]);

    const rows = await live();
    // The §7.5 failure asserted rather than assumed: a round trip through a
    // JSON number would come back with the last few digits changed.
    expect(JSON.parse(rows[0]!.body)).toEqual({ suppliedShares: HUGE_SHARES });

    // The topic words survive verbatim, and a third one that was never indexed
    // reads back null rather than an empty string — ReportDeficit relies on
    // that difference being real.
    const topics = await client.query({
      query: `SELECT topic1, topic3 FROM ${EVENTS_VIEW} LIMIT 1`,
      format: 'JSONEachRow',
    });
    expect(await topics.json()).toEqual([
      { topic1: `0x${'07'.padStart(64, '0')}`, topic3: `0x${'11'.repeat(20).padStart(64, '0')}` },
    ]);
  });

  it('reads back a topic that was never indexed as null', async () => {
    await index(1000, 1000, [event({ topic3: null })]);

    const rows = await client.query({
      query: `SELECT topic3 FROM ${EVENTS_VIEW} LIMIT 1`,
      format: 'JSONEachRow',
    });
    expect(await rows.json()).toEqual([{ topic3: null }]);
  });

  it('is idempotent when the identical range is dispatched again', async () => {
    const events = [event({ logIndex: 0 }), event({ logIndex: 1 })];

    await index(1000, 1100, events);
    await index(1000, 1100, events);

    // Dispatch is at-least-once: a processor returning ok is re-invoked
    // whenever a later one asks to retry. Without retract-before-insert this
    // reads 4, and every count and sum over the table is double.
    expect(await liveCount()).toBe(2);
  });

  it('doubles the range when append runs without the revert before it', async () => {
    const events = [event({ logIndex: 0 }), event({ logIndex: 1 })];

    await index(1000, 1100, events);
    // The mistake the two-method port makes possible. Asserted rather than
    // warned about in a comment: the engine pairs on opposite signs, so two
    // `+1` rows for one key never collapse and every count over them is out by
    // a factor of two, silently.
    await store.append(events);

    expect(await liveCount()).toBe(4);
  });

  it('empties a range when given no events, which is what a reorg does', async () => {
    await index(1000, 1000, [event()]);
    await store.revert(CHAIN_ID, 1000, 1000);

    expect(await liveCount()).toBe(0);
  });

  it('leaves blocks outside the replaced range untouched', async () => {
    await index(900, 900, [event({ blockNumber: 900 })]);
    await index(1000, 1000, [event({ blockNumber: 1000 })]);

    await store.revert(CHAIN_ID, 1000, 1000);

    expect((await live()).map((r) => r.block_number)).toEqual([900]);
  });

  it('drops surplus rows when the new chain emits fewer logs at a block', async () => {
    await index(1000, 1000, [
      event({ logIndex: 0 }),
      event({ logIndex: 1 }),
      event({ logIndex: 2 }),
    ]);

    // The case a version-keyed "latest wins" engine could not handle on its
    // own: log_index 1 and 2 are not re-inserted, so nothing supersedes them
    // and only an explicit retraction removes them.
    await index(1000, 1000, [event({ logIndex: 0 })]);

    expect((await live()).map((r) => r.log_index)).toEqual([0]);
  });

  it('replaces content when a reorg rewrites the same block and log index', async () => {
    await index(1000, 1000, [event({ body: { branch: 'old' } })]);
    await index(1000, 1000, [
      event({ body: { branch: 'new' }, blockHash: `0x${'cc'.repeat(32)}` }),
    ]);

    const rows = await live();
    expect(rows).toHaveLength(1);
    // Grouping the view by version is what makes this the new content rather
    // than an arbitrary pick between the two.
    expect(JSON.parse(rows[0]!.body)).toEqual({ branch: 'new' });
  });

  it('never leaves a key with two live rows', async () => {
    await index(1000, 1000, [event()]);
    await index(1000, 1000, [event()]);
    await index(1000, 1000, [event()]);

    expect(
      await scalar(`SELECT count() AS n FROM (
                      SELECT block_number, log_index FROM ${EVENTS_VIEW}
                      GROUP BY block_number, log_index HAVING count() > 1)`),
    ).toBe(0);
  });

  it('retracts with an exact copy of the row, every column', async () => {
    await index(1000, 1000, [event()]);
    await store.revert(CHAIN_ID, 1000, 1000);

    // Not for the sake of collapsing — the engine pairs on sorting key, version
    // and opposite sign, and would collapse these whatever the other columns
    // said. It matters because a materialized view sees each row as inserted:
    // a retraction missing a column subtracts from the wrong group, so the
    // ledger nets to zero while every projection over it keeps the position.
    expect(
      await scalar(`SELECT count() AS n FROM (
                      SELECT * EXCEPT (sign) FROM ${EVENTS_TABLE} WHERE sign = 1
                      UNION DISTINCT
                      SELECT * EXCEPT (sign) FROM ${EVENTS_TABLE} WHERE sign = -1)`),
    ).toBe(1);
  });

  it('issues no DELETE and no ALTER', async () => {
    await index(1000, 1000, [event()]);
    await store.revert(CHAIN_ID, 1000, 1000);

    // Retraction is an append. If this ever stops holding, the "immutable log"
    // the whole design rests on has quietly stopped being one.
    expect(
      await scalar(`SELECT count() AS n FROM system.mutations WHERE table = '${EVENTS_TABLE}'`),
    ).toBe(0);
  });
});
