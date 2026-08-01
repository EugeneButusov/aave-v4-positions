import type { HealthIndicator } from '@packages/ops';
import { Inject, Injectable } from '@nestjs/common';
import type { Sql } from 'postgres';

import { POSTGRES_CLIENT } from './postgres.options';

/**
 * Reports the indexer's own state store reachable.
 *
 * `SELECT 1`, for the same reason the ClickHouse probe uses one: this answers
 * "can the loop record where it got to at all". A store it cannot reach means
 * every cursor save fails, the range is re-dispatched forever, and the indexer
 * stops making progress — so it belongs in readiness rather than being left to
 * the stall alarm to notice several minutes later.
 */
@Injectable()
export class PostgresHealthIndicator implements HealthIndicator {
  readonly name = 'postgres';

  constructor(@Inject(POSTGRES_CLIENT) private readonly sql: Sql) {}

  async check(): Promise<void> {
    await this.sql`SELECT 1`;
  }
}
