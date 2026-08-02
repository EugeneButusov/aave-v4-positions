import { loadMigrations } from '@packages/migrations';
import { migrate } from '@packages/postgres';
import postgres from 'postgres';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import { ENRICHMENT_MIGRATIONS_DIR } from '../postgres-migrations';
import { PostgresTokenMetadataStore } from './postgres-token-metadata-store';
import type { TokenMetadataRow } from './token-metadata';

const URL = process.env['POSTGRES_URL'] ?? '';
const CHAIN_ID = 1;
const OTHER_CHAIN = 8453;

/** Its own schema, for the reason the cursor store's spec gives. */
const SCHEMA = 'token_metadata_spec';

const USDC = '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48';
const WETH = '0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2';

const sql = postgres(URL, { max: 2, connection: { search_path: SCHEMA } });

/** One row with a distinct value in every column, so a crossed mapping fails. */
function row(over: Partial<TokenMetadataRow> = {}): TokenMetadataRow {
  return {
    chainId: CHAIN_ID,
    token: USDC,
    symbol: 'USDC',
    name: 'USD Coin',
    tokenDecimals: 6,
    fetchedAtBlock: 25_652_782,
    ...over,
  };
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

  await migrate(sql, await loadMigrations([ENRICHMENT_MIGRATIONS_DIR]));
});

afterAll(async () => {
  await sql.end();
});

describe('PostgresTokenMetadataStore', () => {
  let store: PostgresTokenMetadataStore;

  beforeEach(async () => {
    await sql`TRUNCATE token_metadata`;
    store = new PostgresTokenMetadataStore(sql);
  });

  describe('put', () => {
    it('maps every column to its own field', async () => {
      await store.put([row()]);

      // Every value distinct, so crossing two fields in the mapper fails here
      // rather than surfacing as a wrong label much later.
      const [stored] = await sql`SELECT * FROM token_metadata`;
      expect(stored).toMatchObject({
        chain_id: '1',
        token: USDC,
        symbol: 'USDC',
        name: 'USD Coin',
        token_decimals: 6,
        fetched_at_block: '25652782',
      });
    });

    it('replaces in place rather than accumulating', async () => {
      await store.put([row()]);
      await store.put([row({ symbol: 'USDC.e', fetchedAtBlock: 25_700_000 })]);

      // The whole reason this table is in Postgres. A column store answers the
      // same instruction by writing another row and collapsing later.
      const stored = await sql`SELECT symbol, fetched_at_block FROM token_metadata`;
      expect(stored).toHaveLength(1);
      expect(stored[0]).toMatchObject({ symbol: 'USDC.e', fetched_at_block: '25700000' });
    });

    it('stores a token that answered nothing', async () => {
      await store.put([row({ symbol: null, name: null, tokenDecimals: null })]);

      // The row existing is what records that the question was put. Without it
      // a token with no `symbol()` is re-read on every sweep, forever.
      expect((await store.labels(CHAIN_ID)).has(USDC)).toBe(true);
    });

    it('lower-cases the address it is given', async () => {
      // A caller reads a checksummed address off a block explorer, while the
      // Hub fold stores what the log carried, which is lower-case. Writing the
      // checksummed form would join against nothing, forever.
      await store.put([row({ token: '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48' })]);

      expect([...(await store.labels(CHAIN_ID)).keys()]).toEqual([USDC]);
    });

    it('writes nothing, and does not fail, for an empty batch', async () => {
      // A sweep that found no gaps still calls this.
      await expect(store.put([])).resolves.toBeUndefined();
    });
  });

  describe('labels', () => {
    it('omits a token it has never been asked about', async () => {
      await store.put([row()]);

      // Absent rather than present-with-nulls: the caller has to tell "never
      // asked" from "asked, and it has no symbol", because one is a gap to
      // fill and the other is not.
      const labels = await store.labels(CHAIN_ID);
      expect(labels.get(USDC)).toEqual({ symbol: 'USDC', name: 'USD Coin' });
      expect(labels.has(WETH)).toBe(false);
    });

    it('reports a token that answered nothing as present with nulls', async () => {
      await store.put([row({ symbol: null, name: null })]);

      const labels = await store.labels(CHAIN_ID);
      expect(labels.has(USDC)).toBe(true);
      expect(labels.get(USDC)).toEqual({ symbol: null, name: null });
    });

    it('is empty for a chain with nothing stored', async () => {
      // The API asks on every request, including before enrichment has run.
      expect(await store.labels(CHAIN_ID)).toEqual(new Map());
    });
  });

  describe('scope', () => {
    it('keeps one chain apart from another running the same token address', async () => {
      await store.put([row(), row({ chainId: OTHER_CHAIN, symbol: 'USDbC' })]);

      // The same contract address is deployed on more than one chain, so
      // `chain_id` is part of the key rather than a convenience.
      expect((await store.labels(CHAIN_ID)).get(USDC)?.symbol).toBe('USDC');
      expect((await store.labels(OTHER_CHAIN)).get(USDC)?.symbol).toBe('USDbC');
      expect((await store.labels(OTHER_CHAIN)).size).toBe(1);
    });
  });
});
