import { join } from 'node:path';

import { CLICKHOUSE_CLIENT } from '@packages/clickhouse';
import type { ClickHouseClient } from '@clickhouse/client';
import { Inject, Injectable } from '@nestjs/common';

import type { DecodedEvent } from '../decode/decoded-event';
import type { EventStore } from './event-store';
export const EVENTS_TABLE = 'spoke_events';
export const EVENTS_VIEW = 'spoke_events_current';

/**
 * This package's schema, owned here rather than in a central list.
 *
 * A directory rather than the loaded migrations: reading it is filesystem work,
 * and importing this package to use the decoder should not do any. The
 * application passes it to the runner at migration time.
 *
 * The ordinals are unique across every package that contributes migrations, not
 * just within this directory — the runner rejects a clash rather than letting
 * the apply order depend on how the application concatenated the sets.
 */
export const EVENT_MIGRATIONS_DIR = join(__dirname, 'migrations');

/**
 * Every column the retraction has to reproduce, in `001_spoke_events.sql` order.
 *
 * Only {@link RETRACT} reads this — the insert below names its columns as object
 * keys instead.
 *
 * Completeness is not what makes the pair collapse: the engine matches on the
 * sorting key, the version and opposite signs, and collapses them whatever the
 * other columns hold. It matters because a **materialized view sees each row as
 * it is inserted**. A retraction missing a column subtracts from the wrong
 * group — measured: dropping the user column left `alice=100` standing beside
 * `<null>=-100` — so the ledger nets to zero while every projection over it
 * keeps the position.
 */
const COLUMNS = [
  'chain_id',
  'address',
  'block_number',
  'log_index',
  'block_hash',
  'block_timestamp',
  'tx_hash',
  'tx_index',
  'event_name',
  'topic1',
  'topic2',
  'topic3',
  'body',
  'data',
].join(', ');

/**
 * Copies the live rows of a range back in with the sign flipped.
 *
 * Server-side `INSERT ... SELECT` rather than reading rows into Node and
 * writing them back: the retraction has to reproduce **every column and the
 * original version** for the engine to pair it, and selecting from the view is
 * the only way to be sure it does. Round-tripping the rows would be the same
 * query plus a chance to drop a field.
 */
const RETRACT = `
INSERT INTO ${EVENTS_TABLE} (${COLUMNS}, version, sign)
SELECT ${COLUMNS}, version, -1
FROM ${EVENTS_VIEW}
WHERE chain_id = {chainId:UInt32}
  AND block_number BETWEEN {fromBlock:UInt64} AND {toBlock:UInt64}
`;

/**
 * The append-only event log.
 *
 * Nothing here issues a `DELETE` or an `ALTER`. A reorg retracts rows by
 * writing their negation, which is the same mechanism a retry uses, so there is
 * exactly one write path to reason about.
 */
@Injectable()
export class ClickHouseEventStore implements EventStore {
  constructor(@Inject(CLICKHOUSE_CLIENT) private readonly client: ClickHouseClient) {}

  async revert(chainId: number, fromBlock: number, toBlock: number): Promise<void> {
    await this.client.command({
      query: RETRACT,
      query_params: { chainId, fromBlock, toBlock },
    });
  }

  async append(events: readonly DecodedEvent[]): Promise<void> {
    if (events.length === 0) return;

    // One stamp for the whole batch. It only has to differ between successive
    // states of the same log, and two dispatches of one range are separated by
    // at least an RPC round trip. This is not a "latest wins" field — the engine
    // pairs by equality and does not supersede.
    const version = Date.now();

    await this.client.insert({
      table: EVENTS_TABLE,
      format: 'JSONEachRow',
      values: events.map((event) => ({
        chain_id: event.chainId,
        address: event.address,
        block_number: event.blockNumber,
        block_hash: event.blockHash,
        block_timestamp: event.blockTimestamp,
        tx_hash: event.txHash,
        tx_index: event.txIndex,
        log_index: event.logIndex,
        event_name: event.eventName,
        // The indexed parameters as they arrived. Null where the event indexes
        // fewer than three — only ReportDeficit, today.
        topic1: event.topic1,
        topic2: event.topic2,
        topic3: event.topic3,
        // Already a string per uint256 by the time it gets here: JSON parsing
        // would round anything past 2^53 (§7.5).
        body: JSON.stringify(event.body),
        data: event.data,
        version,
        sign: 1,
      })),
    });
  }
}
