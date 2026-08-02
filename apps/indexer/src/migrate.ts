import { EVENT_MIGRATIONS_DIR } from '@aave-positions/events';
import { POSITION_MIGRATIONS_DIR } from '@aave-positions/positions';
import { loadMigrations, migrate } from '@packages/clickhouse';
import { createClient } from '@clickhouse/client';

import { clickHouseEnvSchema } from './config/env';

/**
 * The packages contributing schema to this deployment.
 *
 * The list lives here rather than in the ClickHouse package because the
 * application is what decides which packages ship together. Ordinals are unique
 * across all of them and the runner refuses a set where they are not, which is
 * also what orders these two: the positions package's views read the events
 * package's table, and `010 > 002` is the whole guarantee that they are created
 * after it.
 */
const MIGRATION_DIRS = [EVENT_MIGRATIONS_DIR, POSITION_MIGRATIONS_DIR];

/**
 * Applies pending migrations, then exits.
 *
 * Its own entry point rather than something `AppModule` does at boot: two
 * replicas starting together would race each other through the same DDL.
 * Compose runs it as a one-shot service the indexer waits on; a deployment runs
 * it before rolling pods.
 */
async function main(): Promise<void> {
  const env = clickHouseEnvSchema.parse(process.env);

  const client = createClient({
    url: env.CLICKHOUSE_URL,
    database: env.CLICKHOUSE_DATABASE,
    username: env.CLICKHOUSE_USER,
    password: env.CLICKHOUSE_PASSWORD,
  });

  try {
    const applied = await migrate(client, await loadMigrations(MIGRATION_DIRS));
    console.warn(
      applied.length === 0 ? 'schema already up to date' : `applied: ${applied.join(', ')}`,
    );
  } finally {
    await client.close();
  }
}

main().catch((error: unknown) => {
  console.error(error instanceof Error ? error.message : error);
  process.exitCode = 1;
});
