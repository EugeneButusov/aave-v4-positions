import { Inject, Injectable } from '@nestjs/common';
import { POSTGRES_CLIENT } from '@packages/postgres';
import type { Sql } from 'postgres';

import type { ReservePrice, ReservePriceRow } from './reserve-price';
import { reserveKey, type ReservePriceStore } from './reserve-price-store';

/**
 * `numeric` and `bigint` columns arrive as strings, because postgres.js will not
 * silently round a value past 2^53. Both are what the port promises anyway:
 * `reserve_id` is a `uint256` and `price` is multiplied by an amount before
 * anyone looks at it (§7.1).
 */
interface Row {
  readonly spoke: string;
  readonly reserve_id: string;
  readonly price: string;
  readonly priced_at: Date;
}

/**
 * Reserve prices in Postgres.
 *
 * The second store in this package that writes, and the second not backed by a
 * materialized view — because this data is fetched rather than folded. See
 * `011_reserve_prices.sql` for why it is here rather than in ClickHouse, and
 * why it carries a timestamp rather than a block.
 */
@Injectable()
export class PostgresReservePriceStore implements ReservePriceStore {
  constructor(@Inject(POSTGRES_CLIENT) private readonly sql: Sql) {}

  async latest(chainId: number): Promise<ReadonlyMap<string, ReservePrice>> {
    // Keyed by chain and nothing else, so it does not depend on the page and
    // can be issued beside the ClickHouse query rather than after it.
    const rows = await this.sql<Row[]>`
      SELECT spoke, reserve_id, price, priced_at
      FROM reserve_prices
      WHERE chain_id = ${chainId}
    `;

    return new Map(
      rows.map((row) => [
        reserveKey(row.spoke, row.reserve_id),
        { price: row.price, pricedAt: row.priced_at },
      ]),
    );
  }

  async put(rows: readonly ReservePriceRow[]): Promise<void> {
    if (rows.length === 0) return;

    const values = rows.map((row) => ({
      chain_id: row.chainId,
      // Lower-cased on the way in, not trusted from the caller. The service
      // joins this against the `spoke` the position fold stores, which is
      // `lower()`ed — a checksummed address written here would match nothing
      // and read as a reserve nobody has priced, forever.
      spoke: row.spoke.toLowerCase(),
      reserve_id: row.reserveId,
      price: row.price,
    }));

    // A real upsert, which is the whole reason this table is in Postgres.
    // `priced_at` is the database's own `now()` rather than a value carried in,
    // so a re-read says when it was last asked rather than when some client
    // thought it was.
    await this.sql`
      INSERT INTO reserve_prices ${this.sql(values, 'chain_id', 'spoke', 'reserve_id', 'price')}
      ON CONFLICT (chain_id, spoke, reserve_id) DO UPDATE SET
          price     = EXCLUDED.price,
          priced_at = now()
    `;
  }
}
