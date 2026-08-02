import { Inject, Injectable } from '@nestjs/common';
import { POSTGRES_CLIENT } from '@packages/postgres';
import type { Sql } from 'postgres';

import type { BlockHeader, Hash } from '../../chain/chain-client';
import type { BlockHeaderStore } from './block-header-store';

/** See the note on `CursorRow`: `bigint` arrives as a string. */
interface HeaderRow {
  readonly block_number: string | number;
  readonly hash: Hash;
  readonly parent_hash: Hash;
  readonly block_timestamp: string | number;
}

/**
 * The reorg detector's retention window, in the same database as the cursor.
 *
 * Each port method is one statement, which is the reason this is Postgres and
 * not the event log's engine: the window is a mutable set, and upsert, prune and
 * truncate are all ordinary things to ask of a table with a composite primary
 * key.
 */
@Injectable()
export class PostgresBlockHeaderStore implements BlockHeaderStore {
  constructor(@Inject(POSTGRES_CLIENT) private readonly sql: Sql) {}

  /**
   * No `ORDER BY`. The port promises none — the detector re-sorts, and
   * `ShufflingBlockHeaderStore` exists to prove it does not lean on one — and
   * adding it here would be a guarantee some later caller starts depending on
   * without anything saying it was made.
   */
  async load(chainId: number): Promise<BlockHeader[]> {
    const rows = await this.sql<HeaderRow[]>`
      SELECT block_number, hash, parent_hash, block_timestamp
      FROM indexer_block_headers
      WHERE chain_id = ${chainId}
    `;

    return rows.map((row) => ({
      number: Number(row.block_number),
      hash: row.hash,
      parentHash: row.parent_hash,
      timestamp: Number(row.block_timestamp),
    }));
  }

  /**
   * One statement, which is what the port's "pruning rides along so a database
   * adapter can do both in one statement" asks for. A data-modifying CTE gets
   * it in one round trip and one implicit transaction, where `begin` around two
   * statements would cost three round trips on a path that also runs once per
   * header during a window refill.
   *
   * `AND chain_id` on the DELETE is load-bearing: without it one chain's
   * retention floor wipes every other chain's window, and no single-chain test
   * can see the difference.
   *
   * The DELETE reads the pre-INSERT snapshot, so the row just written is never
   * a candidate for the prune. Harmless because `retainFrom <= header.number`
   * always holds — the port states it as a precondition.
   */
  async append(chainId: number, header: BlockHeader, retainFrom: number): Promise<void> {
    await this.sql`
      WITH retained AS (
        INSERT INTO indexer_block_headers
          (chain_id, block_number, hash, parent_hash, block_timestamp)
        VALUES (
          ${chainId}, ${header.number}, ${header.hash}, ${header.parentHash}, ${header.timestamp}
        )
        ON CONFLICT (chain_id, block_number) DO UPDATE
           SET hash            = EXCLUDED.hash,
               parent_hash     = EXCLUDED.parent_hash,
               block_timestamp = EXCLUDED.block_timestamp
      )
      DELETE FROM indexer_block_headers
      WHERE chain_id = ${chainId} AND block_number < ${retainFrom}
    `;
  }

  /** `-1` clears the window, because every row satisfies `block_number > -1`. */
  async truncate(chainId: number, lastValidBlock: number): Promise<void> {
    await this.sql`
      DELETE FROM indexer_block_headers
      WHERE chain_id = ${chainId} AND block_number > ${lastValidBlock}
    `;
  }
}
