import { loadMigrations } from '@packages/migrations';
import { migrate } from '@packages/postgres';
import postgres from 'postgres';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import { INDEXING_MIGRATIONS_DIR } from '../../postgres-migrations';
import { hashOf } from '../../test-support/fake-chain-client';
import { PostgresCursorStore } from './postgres-cursor-store';
import { PostgresSyncStatusStore } from './postgres-sync-status-store';

const URL = process.env['POSTGRES_URL'] ?? '';
const CHAIN_ID = 1;

/** Its own schema, for the reason the cursor store's spec gives. */
const SCHEMA = 'sync_status_spec';

const sql = postgres(URL, { max: 2, connection: { search_path: SCHEMA } });

beforeAll(async () => {
  // Its own client: `search_path` is a startup parameter, so the connection
  // above cannot be the one that creates the schema it points at.
  const admin = postgres(URL, { max: 1 });
  try {
    await admin`CREATE SCHEMA IF NOT EXISTS ${admin(SCHEMA)}`;
  } finally {
    await admin.end();
  }

  await migrate(sql, await loadMigrations([INDEXING_MIGRATIONS_DIR]));
});

afterAll(async () => {
  await sql.end();
});

describe('PostgresSyncStatusStore', () => {
  let store: PostgresSyncStatusStore;

  beforeEach(async () => {
    await sql`TRUNCATE indexer_cursor`;
    store = new PostgresSyncStatusStore(sql);
  });

  it('answers null for a chain that has never been indexed', async () => {
    // Which is what lets a caller tell "this deployment does not follow that
    // chain" apart from "that wallet holds nothing" — the same empty list
    // otherwise means both.
    await expect(store.get(137)).resolves.toBeNull();
  });

  it('reports where the indexer got to', async () => {
    await new PostgresCursorStore(sql).save({
      chainId: CHAIN_ID,
      lastBlock: 24_720_899,
      lastHash: hashOf('a', 24_720_899),
    });

    const status = await store.get(CHAIN_ID);

    expect(status).toMatchObject({
      chainId: CHAIN_ID,
      lastBlock: 24_720_899,
      lastHash: hashOf('a', 24_720_899),
    });
    expect(status?.updatedAt).toBeInstanceOf(Date);
  });

  it('returns numbers, not the strings the driver reads bigints as', async () => {
    // Palm's chain id, which does not fit in int4, and a height an L2 reaches
    // in under two decades. postgres.js hands `bigint` back as a string rather
    // than rounding past 2^53, so a payload built straight from the row would
    // carry `lastBlock: "1099511627776"` — a string where the contract says a
    // number, and JSON that no client would parse as a height.
    await new PostgresCursorStore(sql).save({
      chainId: 11_297_108_109,
      lastBlock: 2 ** 40,
      lastHash: hashOf('a', 1),
    });

    const status = await store.get(11_297_108_109);

    expect(status?.lastBlock).toBe(2 ** 40);
    expect(typeof status?.lastBlock).toBe('number');
    expect(typeof status?.chainId).toBe('number');
    expect(typeof status?.ageSeconds).toBe('number');
  });

  it('measures age from the row, on the server that wrote it', async () => {
    await new PostgresCursorStore(sql).save({
      chainId: CHAIN_ID,
      lastBlock: 500,
      lastHash: hashOf('a', 500),
    });

    await expect(store.get(CHAIN_ID)).resolves.toMatchObject({
      ageSeconds: expect.closeTo(0, 0),
    });

    await sql`UPDATE indexer_cursor SET updated_at = now() - interval '90 seconds'`;

    // Computed by the database rather than from `updatedAt` and a local clock:
    // the timestamp is the server's own now(), so a reader with a fast clock
    // would report a healthy indexer as stale, and one with a slow clock would
    // hide a stalled one.
    const stale = await store.get(CHAIN_ID);
    expect(stale?.ageSeconds).toBeGreaterThanOrEqual(90);
    expect(stale?.ageSeconds).toBeLessThan(95);
  });
});
