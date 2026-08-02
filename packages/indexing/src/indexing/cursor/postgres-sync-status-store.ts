import { Inject, Injectable } from '@nestjs/common';
import { POSTGRES_CLIENT } from '@packages/postgres';
import type { Sql } from 'postgres';

import type { Hash } from '../../chain/chain-client';
import type { SyncStatus, SyncStatusStore } from './sync-status-store';

/**
 * `bigint` and `numeric` columns arrive as strings, because postgres.js will
 * not silently round a value past 2^53. The port promises `number`, so both go
 * through `Number()` — declared as the union rather than asserted, so the
 * coercion cannot be forgotten.
 */
interface StatusRow {
  readonly chain_id: string | number;
  readonly last_block: string | number;
  readonly last_hash: Hash;
  readonly updated_at: Date;
  readonly age_seconds: string | number;
}

/**
 * The indexer's position, read back by whoever is serving from it.
 *
 * Writes nothing. It shares a table with {@link PostgresCursorStore} rather
 * than duplicating one, because there is exactly one answer to "where is the
 * indexer" and a second copy of it would be a second thing to keep in step.
 */
@Injectable()
export class PostgresSyncStatusStore implements SyncStatusStore {
  constructor(@Inject(POSTGRES_CLIENT) private readonly sql: Sql) {}

  async get(chainId: number): Promise<SyncStatus | null> {
    const [row] = await this.sql<StatusRow[]>`
      SELECT
          chain_id,
          last_block,
          last_hash,
          updated_at,
          -- Computed here, not by the reader. \`updated_at\` is the database's
          -- own now(), so a reader subtracting it from its own clock reports
          -- skew as staleness — and a fast reader would call a healthy indexer
          -- stale.
          EXTRACT(EPOCH FROM (now() - updated_at)) AS age_seconds
      FROM indexer_cursor
      WHERE chain_id = ${chainId}
    `;

    if (!row) return null;

    return {
      chainId: Number(row.chain_id),
      lastBlock: Number(row.last_block),
      lastHash: row.last_hash,
      updatedAt: row.updated_at,
      ageSeconds: Number(row.age_seconds),
    };
  }
}
