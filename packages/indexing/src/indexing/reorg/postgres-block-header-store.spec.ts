import { loadMigrations } from '@packages/migrations';
import { migrate } from '@packages/postgres';
import postgres from 'postgres';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import type { BlockHeader } from '../../chain/chain-client';
import { INDEXING_MIGRATIONS_DIR } from '../../postgres-migrations';
import { describeBlockHeaderStoreContract } from '../../test-support/block-header-store-contract';
import { hashOf } from '../../test-support/fake-chain-client';
import { PostgresBlockHeaderStore } from './postgres-block-header-store';

const URL = process.env['POSTGRES_URL'] ?? '';
const CHAIN_ID = 1;
const KEEP_ALL = 0;

/** Its own, for the reason given in the cursor store's spec. */
const SCHEMA = 'header_store_spec';

const sql = postgres(URL, { max: 2, connection: { search_path: SCHEMA } });

function header(blockNumber: number, branch = 'a'): BlockHeader {
  return {
    number: blockNumber,
    hash: hashOf(branch, blockNumber),
    parentHash: hashOf(branch, blockNumber - 1),
    timestamp: 1_700_000_000 + blockNumber * 12,
  };
}

async function rowsAt(blockNumber: number): Promise<number> {
  const [row] = await sql<{ count: string }[]>`
    SELECT count(*) AS count FROM indexer_block_headers
    WHERE chain_id = ${CHAIN_ID} AND block_number = ${blockNumber}
  `;
  return Number(row?.count ?? 0);
}

beforeAll(async () => {
  // `search_path` is a startup parameter, so the connection above cannot be the
  // one that creates the schema it points at.
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

describeBlockHeaderStoreContract('PostgresBlockHeaderStore', {
  fresh: async () => {
    await sql`TRUNCATE indexer_block_headers`;
    return new PostgresBlockHeaderStore(sql);
  },
});

describe('PostgresBlockHeaderStore — beyond the contract', () => {
  beforeEach(async () => {
    await sql`TRUNCATE indexer_block_headers`;
  });

  it('leaves one row after the same height is written three times', async () => {
    const store = new PostgresBlockHeaderStore(sql);

    await store.append(CHAIN_ID, header(500, 'a'), KEEP_ALL);
    await store.append(CHAIN_ID, header(500, 'b'), KEEP_ALL);
    await store.append(CHAIN_ID, header(500, 'c'), KEEP_ALL);

    // Asserted against the table rather than through `load`, because `load`
    // would hide a duplicate behind whichever row came back first. The upsert
    // is the ON CONFLICT clause, and the ON CONFLICT clause is the primary key.
    expect(await rowsAt(500)).toBe(1);
    await expect(store.load(CHAIN_ID)).resolves.toEqual([header(500, 'c')]);
  });

  it('returns numbers, not the strings the driver reads bigints as', async () => {
    const store = new PostgresBlockHeaderStore(sql);
    const high = header(2 ** 40);

    await store.append(CHAIN_ID, high, KEEP_ALL);

    const [loaded] = await store.load(CHAIN_ID);
    expect(loaded).toEqual(high);
    expect(typeof loaded?.number).toBe('number');
    expect(typeof loaded?.timestamp).toBe('number');
  });

  it('refuses a hash that is not lowercase 0x-hex', async () => {
    const store = new PostgresBlockHeaderStore(sql);

    await expect(
      store.append(CHAIN_ID, { ...header(500), hash: `0x${'A'.repeat(64)}` }, KEEP_ALL),
    ).rejects.toThrow(/indexer_block_headers_hash_check/);
  });
});
