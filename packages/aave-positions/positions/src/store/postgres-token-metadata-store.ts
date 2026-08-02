import { Inject, Injectable } from '@nestjs/common';
import type { Address } from '@packages/indexing';
import { POSTGRES_CLIENT } from '@packages/postgres';
import type { Sql } from 'postgres';

import type { TokenLabel, TokenMetadataRow } from './token-metadata';
import type { TokenMetadataStore } from './token-metadata-store';

/**
 * `bigint` columns arrive as strings, because postgres.js will not silently
 * round a value past 2^53. `chain_id` is one, and the port promises `number`.
 */
interface Row {
  readonly chain_id: string | number;
  readonly token: Address;
  readonly symbol: string | null;
  readonly name: string | null;
}

/**
 * Token metadata in Postgres.
 *
 * **The one store in this package that writes**, and the only one not backed by
 * a materialized view — because this data is fetched rather than folded. See
 * `010_token_metadata.sql` for why it is here rather than in ClickHouse, with
 * the measurements.
 */
@Injectable()
export class PostgresTokenMetadataStore implements TokenMetadataStore {
  constructor(@Inject(POSTGRES_CLIENT) private readonly sql: Sql) {}

  async labels(
    chainId: number,
    tokens: readonly Address[],
  ): Promise<ReadonlyMap<Address, TokenLabel>> {
    // An empty page asks for nothing. Left to the query this becomes
    // `IN ()`, which is a syntax error rather than an empty result.
    if (tokens.length === 0) return new Map();

    const lowered = tokens.map((token) => token.toLowerCase());
    const rows = await this.sql<Row[]>`
      SELECT chain_id, token, symbol, name
      FROM token_metadata
      WHERE chain_id = ${chainId} AND token = ANY(${this.sql.array([...lowered])})
    `;

    // A token with no row is simply not in the map. The caller distinguishes
    // "never asked" from "asked, and there is no symbol" by presence, which is
    // the distinction the nullable columns exist to preserve.
    return new Map(rows.map((row) => [row.token, { symbol: row.symbol, name: row.name }]));
  }

  async known(chainId: number): Promise<ReadonlySet<Address>> {
    const rows = await this.sql<{ token: Address }[]>`
      SELECT token FROM token_metadata WHERE chain_id = ${chainId}
    `;
    return new Set(rows.map((row) => row.token));
  }

  async put(rows: readonly TokenMetadataRow[]): Promise<void> {
    if (rows.length === 0) return;

    const values = rows.map((row) => ({
      chain_id: row.chainId,
      // Lower-cased on the way in, not trusted from the caller. The join on the
      // read side compares against `hub_assets_current.underlying`, which the
      // fold stores `lower()`ed — a checksummed address written here would
      // match nothing and read as a token nobody has enriched yet, forever.
      token: row.token.toLowerCase(),
      symbol: row.symbol,
      name: row.name,
      token_decimals: row.tokenDecimals,
      fetched_at_block: row.fetchedAtBlock,
    }));

    // A real upsert, which is the whole reason this table is in Postgres. The
    // timestamp is the database's own, so re-running says when it last asked
    // rather than when some client thought it did.
    await this.sql`
      INSERT INTO token_metadata ${this.sql(
        values,
        'chain_id',
        'token',
        'symbol',
        'name',
        'token_decimals',
        'fetched_at_block',
      )}
      ON CONFLICT (chain_id, token) DO UPDATE SET
          symbol           = EXCLUDED.symbol,
          name             = EXCLUDED.name,
          token_decimals   = EXCLUDED.token_decimals,
          fetched_at       = now(),
          fetched_at_block = EXCLUDED.fetched_at_block
    `;
  }
}
