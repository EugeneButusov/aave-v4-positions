import { createClient, type ClickHouseClient } from '@clickhouse/client';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import type { DecodedEvent } from '../decode/decoded-event';
import { applySql } from './apply-sql';
import { EVENT_MIGRATIONS_DIR } from './clickhouse-event-store';
import { ClickHouseHubEventStore } from './clickhouse-hub-event-store';
import { ClickHouseSpokeEventStore } from './clickhouse-spoke-event-store';

/** Its own database, so a sibling suite cannot truncate these tables mid-run. */
const DATABASE = 'spec_hub_events';
const CONNECTION = {
  url: process.env['CLICKHOUSE_URL'] ?? 'http://localhost:8123',
  username: process.env['CLICKHOUSE_USER'] ?? 'default',
  password: process.env['CLICKHOUSE_PASSWORD'] ?? '',
};
const CHAIN_ID = 1;
const HUB = '0xcca852bc40e560adc3b1cc58ca5b55638ce826c9';
const SPOKE = '0x94e7a5dcbe816e498b89ab752661904e2f56c485';
/** A RAY-scaled index: past 2^53 on its first day, which is the point. */
const DRAWN_INDEX = '1000000000000000000000000000';

let client: ClickHouseClient;
let hub: ClickHouseHubEventStore;
let spoke: ClickHouseSpokeEventStore;

function updateAsset(overrides: Partial<DecodedEvent> = {}): DecodedEvent {
  return {
    chainId: CHAIN_ID,
    address: HUB,
    blockNumber: 1000,
    blockHash: `0x${'aa'.repeat(32)}`,
    blockTimestamp: 1_785_000_000,
    txHash: `0x${'bb'.repeat(32)}`,
    txIndex: 0,
    logIndex: 0,
    eventName: 'UpdateAsset',
    topic1: `0x${'07'.padStart(64, '0')}`,
    topic2: null,
    topic3: null,
    body: { assetId: '7', drawnIndex: DRAWN_INDEX, drawnRate: '0', accruedFees: '0' },
    data: '0x00',
    ...overrides,
  };
}

const countIn = async (table: string): Promise<number> => {
  const result = await client.query({
    query: `SELECT count() AS n FROM ${table}`,
    format: 'JSONEachRow',
  });
  return Number((await result.json<{ n: string | number }>())[0]?.n);
};

describe('ClickHouseHubEventStore', () => {
  beforeAll(async () => {
    const bootstrap = createClient(CONNECTION);
    await bootstrap.command({ query: `CREATE DATABASE IF NOT EXISTS ${DATABASE}` });
    await bootstrap.close();

    client = createClient({ ...CONNECTION, database: DATABASE });
    await applySql(client, [EVENT_MIGRATIONS_DIR]);
    hub = new ClickHouseHubEventStore(client);
    spoke = new ClickHouseSpokeEventStore(client);
  });

  afterAll(async () => {
    await client.close();
  });

  beforeEach(async () => {
    await client.command({ query: `TRUNCATE TABLE hub_events` });
    await client.command({ query: `TRUNCATE TABLE spoke_events` });
  });

  it('writes to its own ledger and leaves the Spoke ledger alone', async () => {
    await hub.append([updateAsset()]);

    // The two subclasses share every line of the write path; the table name is
    // the whole difference, and it is the difference that matters.
    expect(await countIn('hub_events_current')).toBe(1);
    expect(await countIn('spoke_events_current')).toBe(0);
  });

  it('retracts only its own range, not the Spoke rows in the same blocks', async () => {
    await hub.append([updateAsset()]);
    await spoke.append([
      {
        ...updateAsset(),
        address: SPOKE,
        eventName: 'Supply',
        body: { reserveId: '7', user: SPOKE, suppliedShares: '5', suppliedAmount: '5' },
      },
    ]);

    await hub.revert(CHAIN_ID, 900, 1100);

    // Both ledgers hold block 1000. A revert that reached across would silently
    // erase the other stream every time a range was re-dispatched.
    expect(await countIn('hub_events_current')).toBe(0);
    expect(await countIn('spoke_events_current')).toBe(1);
  });

  it('survives the same range being dispatched twice', async () => {
    await hub.revert(CHAIN_ID, 900, 1100);
    await hub.append([updateAsset()]);
    await hub.revert(CHAIN_ID, 900, 1100);
    await hub.append([updateAsset()]);

    // Dispatch is at-least-once. Revert-then-append is what keeps a second pass
    // from leaving two live copies.
    expect(await countIn('hub_events_current')).toBe(1);
  });

  it('keeps a RAY-scaled index exact', async () => {
    await hub.append([updateAsset()]);

    const result = await client.query({
      query: `SELECT JSONExtractString(body, 'drawnIndex') AS i FROM hub_events_current`,
      format: 'JSONEachRow',
    });

    // Round-tripping this through a JSON number would lose the tail, and every
    // debt valuation with it (§7.5).
    expect((await result.json<{ i: string }>())[0]?.i).toBe(DRAWN_INDEX);
  });
});
