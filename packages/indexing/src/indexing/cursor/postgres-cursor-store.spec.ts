import { loadMigrations } from '@packages/migrations';
import { migrate } from '@packages/postgres';
import postgres from 'postgres';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import { INDEXING_MIGRATIONS_DIR } from '../../postgres-migrations';
import { describeCursorStoreContract } from '../../test-support/cursor-store-contract';
import { hashOf } from '../../test-support/fake-chain-client';
import { PostgresBlockHeaderStore } from '../reorg/postgres-block-header-store';
import type { Cursor } from './cursor-store';
import { PostgresCursorStore } from './postgres-cursor-store';

const URL = process.env['POSTGRES_URL'] ?? '';
const CHAIN_ID = 1;

/**
 * A schema of its own. These specs truncate between tests, and this package's
 * other spec files now run alongside them against one server — in a shared
 * `public` they would be clearing each other's fixtures, intermittently and
 * only under load.
 */
const SCHEMA = 'cursor_store_spec';
const connection = { search_path: SCHEMA };

const sql = postgres(URL, { max: 2, connection });

function cursorAt(lastBlock: number, chainId = CHAIN_ID): Cursor {
  return { chainId, lastBlock, lastHash: hashOf('a', lastBlock) };
}

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

describeCursorStoreContract('PostgresCursorStore', {
  fresh: async () => {
    await sql`TRUNCATE indexer_cursor`;
    return new PostgresCursorStore(sql);
  },
});

describe('PostgresCursorStore — beyond the contract', () => {
  beforeEach(async () => {
    await sql`TRUNCATE indexer_cursor, indexer_block_headers`;
  });

  it('is read back by a connection that did not write it', async () => {
    await new PostgresCursorStore(sql).save(cursorAt(24_720_899));

    const reader = postgres(URL, { max: 1, connection });
    try {
      // The whole point of the change, and the one assertion the in-memory
      // adapter can never make: a different process resumes where this one got
      // to instead of starting again at INDEXER_START_BLOCK.
      await expect(new PostgresCursorStore(reader).load(CHAIN_ID)).resolves.toEqual(
        cursorAt(24_720_899),
      );
    } finally {
      await reader.end();
    }
  });

  it('returns numbers, not the strings the driver reads bigints as', async () => {
    // Palm's chain id, which does not fit in int4, and a height an L2 reaches in
    // under two decades. postgres.js hands `bigint` back as a string rather than
    // silently rounding past 2^53, so an adapter that passed the row through
    // would answer `lastBlock: "1099511627776"` — and `lastBlock + 1` would be
    // string concatenation, sending the loop to block 10995116277761.
    const cursor = cursorAt(2 ** 40, 11_297_108_109);
    await new PostgresCursorStore(sql).save(cursor);

    const loaded = await new PostgresCursorStore(sql).load(11_297_108_109);

    expect(loaded).toEqual(cursor);
    expect(typeof loaded?.lastBlock).toBe('number');
    expect(typeof loaded?.chainId).toBe('number');
  });

  it('refuses a hash that is not lowercase 0x-hex', async () => {
    const store = new PostgresCursorStore(sql);

    // Loud beats silent. The detector compares hashes with `!==`, so an
    // upper-cased hash would never match the chain's answer and every restart
    // would report an unrecoverable reorg — a wrong answer with a plausible
    // shape, where a rejected write is a stack trace naming the constraint.
    await expect(
      store.save({ chainId: CHAIN_ID, lastBlock: 500, lastHash: `0x${'A'.repeat(64)}` }),
    ).rejects.toThrow(/indexer_cursor_last_hash_check/);
  });

  it('leaves the window at or above the cursor across a restart', async () => {
    const cursors = new PostgresCursorStore(sql);
    const headers = new PostgresBlockHeaderStore(sql);
    const header = {
      number: 500,
      hash: hashOf('a', 500),
      parentHash: hashOf('a', 499),
      timestamp: 1_700_000_000,
    };

    // The loop's order: commit the header, then save the cursor. Nothing wraps
    // the two, and nothing needs to — this ordering is what guarantees the
    // window is never behind the cursor, whichever of them a crash lands
    // between, and a window behind its cursor is the one state bootstrap calls
    // corruption rather than a fork.
    await headers.append(CHAIN_ID, header, 372);
    await cursors.save(cursorAt(500));

    const reader = postgres(URL, { max: 1, connection });
    try {
      const window = await new PostgresBlockHeaderStore(reader).load(CHAIN_ID);
      const resumed = await new PostgresCursorStore(reader).load(CHAIN_ID);
      const top = Math.max(...window.map((entry) => entry.number));

      expect(top).toBeGreaterThanOrEqual(resumed?.lastBlock ?? -1);
    } finally {
      await reader.end();
    }
  });
});
