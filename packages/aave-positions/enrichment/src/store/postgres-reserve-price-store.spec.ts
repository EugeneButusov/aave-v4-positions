import { loadMigrations } from '@packages/migrations';
import { migrate } from '@packages/postgres';
import postgres from 'postgres';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import { ENRICHMENT_MIGRATIONS_DIR } from '../postgres-migrations';
import { PostgresReservePriceStore } from './postgres-reserve-price-store';
import type { ReservePriceRow } from './reserve-price';
import { reserveKey } from './reserve-price-store';

const URL = process.env['POSTGRES_URL'] ?? '';
const CHAIN_ID = 1;
const OTHER_CHAIN = 8453;

/** Its own schema, for the reason the cursor store's spec gives. */
const SCHEMA = 'reserve_prices_spec';

const SPOKE = '0x94e7a5dcbe816e498b89ab752661904e2f56c485';
const OTHER_SPOKE = '0x973a023a77420ba610f06b3858ad991df6d85a08';

const sql = postgres(URL, { max: 2, connection: { search_path: SCHEMA } });

function row(over: Partial<ReservePriceRow> = {}): ReservePriceRow {
  return { chainId: CHAIN_ID, spoke: SPOKE, reserveId: '0', price: '187522000000', ...over };
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

describe('PostgresReservePriceStore', () => {
  let store: PostgresReservePriceStore;

  beforeEach(async () => {
    await sql`TRUNCATE reserve_prices`;
    store = new PostgresReservePriceStore(sql);
  });

  describe('put', () => {
    it('round-trips a price under the key the read path joins on', async () => {
      await store.put([row()]);

      const prices = await store.latest(CHAIN_ID);

      expect(prices.get(reserveKey(SPOKE, '0'))?.price).toBe('187522000000');
    });

    it('replaces in place rather than accumulating', async () => {
      // The whole reason this table is in Postgres. A ReplacingMergeTree
      // carrying the same data was measured at 51 rows in 3 parts after 103
      // upserts; here the row count is the answer.
      for (let n = 1; n <= 20; n += 1) {
        // oxlint-disable-next-line no-await-in-loop
        await store.put([row({ price: String(n) })]);
      }

      const counted = await sql<{ count: string }[]>`SELECT count(*) FROM reserve_prices`;
      expect(counted[0]?.count).toBe('1');
      expect((await store.latest(CHAIN_ID)).get(reserveKey(SPOKE, '0'))?.price).toBe('20');
    });

    it('lower-cases the spoke on the way in', async () => {
      // The service joins this against the `spoke` the position fold stores,
      // which is `lower()`ed. A checksummed address written here would match
      // nothing and read as a reserve nobody has priced, forever.
      await store.put([row({ spoke: '0x94E7A5dCbE816e498b89aB752661904E2F56c485' })]);

      const prices = await store.latest(CHAIN_ID);

      expect(prices.has(reserveKey(SPOKE, '0'))).toBe(true);
    });

    it('treats a reserve id as a number, not as digits', async () => {
      // Why the column is `numeric` rather than `text`: one canonical spelling
      // per value, so a caller that pads cannot create a second row for one
      // reserve — and cannot leave a price the read path never finds.
      await store.put([row({ reserveId: '13', price: '100' })]);
      await store.put([row({ reserveId: '013', price: '200' })]);

      const prices = await store.latest(CHAIN_ID);

      expect(prices.get(reserveKey(SPOKE, '13'))?.price).toBe('200');
      expect(prices.size).toBe(1);
    });

    it('keeps two spokes apart', async () => {
      // A Spoke is an isolated margin account with its own oracle (§12.3), so
      // the same reserve id on two spokes is two unrelated prices.
      await store.put([row({ price: '100' }), row({ spoke: OTHER_SPOKE, price: '200' })]);

      const prices = await store.latest(CHAIN_ID);

      expect(prices.get(reserveKey(SPOKE, '0'))?.price).toBe('100');
      expect(prices.get(reserveKey(OTHER_SPOKE, '0'))?.price).toBe('200');
    });

    it('does nothing when given nothing', async () => {
      await expect(store.put([])).resolves.toBeUndefined();
    });

    it('leaves a reserve it was not given alone', async () => {
      // **The rule that inverts `token_metadata`'s.** There a row is written
      // even when every field is null, because a null is the token's answer.
      // Here a reserve the oracle refused is simply absent from the write, so
      // the last good price stays put and ages visibly instead of blanking a
      // USD value on a live endpoint.
      await store.put([
        row({ reserveId: '0', price: '100' }),
        row({ reserveId: '1', price: '200' }),
      ]);

      await store.put([row({ reserveId: '0', price: '111' })]);

      const prices = await store.latest(CHAIN_ID);
      expect(prices.get(reserveKey(SPOKE, '0'))?.price).toBe('111');
      expect(prices.get(reserveKey(SPOKE, '1'))?.price).toBe('200');
    });

    it('stamps the write with the database clock, not the caller', async () => {
      await store.put([row()]);
      const first = (await store.latest(CHAIN_ID)).get(reserveKey(SPOKE, '0'))?.pricedAt;

      await sql`SELECT pg_sleep(0.01)`;
      await store.put([row({ price: '999' })]);
      const second = (await store.latest(CHAIN_ID)).get(reserveKey(SPOKE, '0'))?.pricedAt;

      // Freshness is judged against the server that wrote it. A timestamp
      // carried in from the caller would report the reader's clock skew as age.
      expect(second?.getTime()).toBeGreaterThan(first?.getTime() ?? 0);
    });

    it('refuses a non-positive price at the column', async () => {
      // The oracle reverts rather than answer zero (§7.4), so a zero could only
      // come from this codebase. The CHECK is the backstop that keeps the
      // invention out of the table.
      await expect(store.put([row({ price: '0' })])).rejects.toThrow(/violates check constraint/);
    });
  });

  describe('latest', () => {
    it('is scoped to one chain', async () => {
      await store.put([row({ price: '100' }), row({ chainId: OTHER_CHAIN, price: '200' })]);

      const prices = await store.latest(CHAIN_ID);

      expect(prices.size).toBe(1);
      expect(prices.get(reserveKey(SPOKE, '0'))?.price).toBe('100');
    });

    it('is empty rather than absent when nothing has been priced', async () => {
      // A reserve with no row serves null on the wire. The read path has to be
      // able to tell that from a zero, which would be indistinguishable from a
      // real price of nothing.
      expect((await store.latest(CHAIN_ID)).size).toBe(0);
    });
  });
});
