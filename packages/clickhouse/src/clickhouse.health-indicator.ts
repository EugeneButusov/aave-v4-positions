import type { ClickHouseClient } from '@clickhouse/client';
import type { HealthIndicator } from '@packages/ops';
import { Inject, Injectable } from '@nestjs/common';

import { CLICKHOUSE_CLIENT } from './clickhouse.options';

/**
 * Reports the event store reachable.
 *
 * A plain `SELECT 1` rather than anything about row counts or lag: this answers
 * "can the indexer write at all", and a store it cannot reach is a readiness
 * problem. Whether the *data* is right is reconciliation's question (§9), and
 * it needs its own signal rather than being folded in here.
 */
@Injectable()
export class ClickHouseHealthIndicator implements HealthIndicator {
  readonly name = 'clickhouse';

  constructor(@Inject(CLICKHOUSE_CLIENT) private readonly client: ClickHouseClient) {}

  async check(): Promise<void> {
    const result = await this.client.query({ query: 'SELECT 1', format: 'JSONEachRow' });
    await result.json();
  }
}
