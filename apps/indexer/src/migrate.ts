import { EVENT_MIGRATIONS_DIR } from '@aave-positions/events';
import {
  POSITION_MIGRATIONS_DIR,
  POSITION_POSTGRES_MIGRATIONS_DIR,
} from '@aave-positions/positions';
import { migrate as migrateClickHouse } from '@packages/clickhouse';
import { INDEXING_MIGRATIONS_DIR } from '@packages/indexing';
import { loadMigrations } from '@packages/migrations';
import { migrate as migratePostgres } from '@packages/postgres';
import { createClient } from '@clickhouse/client';
import postgres from 'postgres';

import { clickHouseEnvSchema, postgresEnvSchema } from './config/env';

/**
 * The packages contributing schema to this deployment, one list per database.
 *
 * The lists live here rather than in the database packages because the
 * application is what decides which packages ship together. Within one database
 * the ordinals are unique and the runner refuses a set where they are not, which
 * is also what orders the ClickHouse pair: the positions package's views read the
 * events package's table, and `010 > 002` is the whole guarantee that they are
 * created after it.
 *
 * **Two lists, never one.** `loadMigrations` asserts that ordinals are unique
 * across whatever it is handed, so calling it twice is exactly what keeps the
 * event log's `001_spoke_events` from colliding with the cursor's
 * `001_indexer_cursor`. Concatenating them to run everything in one go would
 * reintroduce the collision and then try to apply Postgres DDL to ClickHouse.
 */
const CLICKHOUSE_MIGRATION_DIRS = [EVENT_MIGRATIONS_DIR, POSITION_MIGRATIONS_DIR];
const POSTGRES_MIGRATION_DIRS = [INDEXING_MIGRATIONS_DIR, POSITION_POSTGRES_MIGRATIONS_DIR];

function report(database: string, applied: string[]): void {
  console.warn(
    applied.length === 0
      ? `${database}: schema already up to date`
      : `${database}: applied ${applied.join(', ')}`,
  );
}

async function migrateClickHouseSchema(): Promise<void> {
  const env = clickHouseEnvSchema.parse(process.env);

  const client = createClient({
    url: env.CLICKHOUSE_URL,
    database: env.CLICKHOUSE_DATABASE,
    username: env.CLICKHOUSE_USER,
    password: env.CLICKHOUSE_PASSWORD,
  });

  try {
    report(
      'clickhouse',
      await migrateClickHouse(client, await loadMigrations(CLICKHOUSE_MIGRATION_DIRS)),
    );
  } finally {
    await client.close();
  }
}

async function migratePostgresSchema(): Promise<void> {
  const env = postgresEnvSchema.parse(process.env);
  const sql = postgres(env.POSTGRES_URL, {
    max: 1,
    // The ledger's `CREATE TABLE IF NOT EXISTS` is applied unconditionally, so
    // 42P07 — "relation already exists, skipping" — is what every run after the
    // first produces. postgres.js would otherwise dump the whole notice object
    // to stdout, which makes a successful migration look like a failure to
    // anyone reading the compose output. Everything else still gets through.
    onnotice: (notice) => {
      if (notice['code'] === '42P07') return;
      console.warn(`postgres: ${notice['message'] ?? JSON.stringify(notice)}`);
    },
  });

  try {
    report('postgres', await migratePostgres(sql, await loadMigrations(POSTGRES_MIGRATION_DIRS)));
  } finally {
    // Not optional. postgres.js keeps its sockets referenced, so without this
    // the process never exits, compose's one-shot service never reports
    // `service_completed_successfully`, and the indexer never starts.
    await sql.end();
  }
}

/**
 * Applies pending migrations to both databases, then exits.
 *
 * Its own entry point rather than something `AppModule` does at boot: two
 * replicas starting together would race each other through the same DDL.
 * Compose runs it as a one-shot service the indexer waits on; a deployment runs
 * it before rolling pods.
 *
 * Sequential, and each half logs under its own name, so a failure says which
 * database it was rather than leaving that to be inferred from the stack.
 */
async function main(): Promise<void> {
  await migrateClickHouseSchema();
  await migratePostgresSchema();
}

main().catch((error: unknown) => {
  console.error(error instanceof Error ? error.message : error);
  process.exitCode = 1;
});
