import { Inject, Injectable } from '@nestjs/common';
import { POSTGRES_CLIENT } from '@packages/postgres';
import type { Sql } from 'postgres';

import type { Hash } from '../../chain/chain-client';
import type { Cursor, CursorStore } from './cursor-store';

/**
 * `bigint` columns arrive as strings, because postgres.js will not silently
 * round a value past 2^53. Block heights are far below that, and the port
 * promises `number`, so every one goes through `Number()` — declared as the
 * union rather than asserted, so the coercion cannot be forgotten.
 */
interface CursorRow {
  readonly chain_id: string | number;
  readonly last_block: string | number;
  readonly last_hash: Hash;
}

/**
 * The cursor, in the database the indexer keeps its own state in.
 *
 * Writes exactly one row per chain and never buffers: the loop commits the
 * header to the window and *then* saves the cursor, so a crash between the two
 * leaves the window at or ahead of the cursor — which is the state
 * `bootstrap` is built to resolve. A write-behind adapter would break that
 * ordering and, with it, the only thing that makes a resume vettable.
 */
@Injectable()
export class PostgresCursorStore implements CursorStore {
  constructor(@Inject(POSTGRES_CLIENT) private readonly sql: Sql) {}

  async load(chainId: number): Promise<Cursor | null> {
    const [row] = await this.sql<CursorRow[]>`
      SELECT chain_id, last_block, last_hash
      FROM indexer_cursor
      WHERE chain_id = ${chainId}
    `;

    if (!row) return null;

    return {
      chainId: Number(row.chain_id),
      lastBlock: Number(row.last_block),
      lastHash: row.last_hash,
    };
  }

  /**
   * **No monotonic guard, deliberately.** `ON CONFLICT ... WHERE last_block <
   * EXCLUDED.last_block` is the integrity check this statement looks like it is
   * missing, and adding it would break every reorg: `applyReorg` saves the last
   * *valid* block, which is below where the cursor already stood. The guarded
   * write would report success, discard the rewind, and leave the loop
   * continuing from a branch that lost.
   */
  async save(cursor: Cursor): Promise<void> {
    await this.sql`
      INSERT INTO indexer_cursor (chain_id, last_block, last_hash, updated_at)
      VALUES (${cursor.chainId}, ${cursor.lastBlock}, ${cursor.lastHash}, now())
      ON CONFLICT (chain_id) DO UPDATE
         SET last_block = EXCLUDED.last_block,
             last_hash  = EXCLUDED.last_hash,
             updated_at = EXCLUDED.updated_at
    `;
  }
}
