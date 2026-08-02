import { join } from 'node:path';

import type { ClickHouseClient } from '@clickhouse/client';

import { loadMigrations, ordered, type Migration } from './migration';

const LEDGER = 'schema_migrations';

/** This package's own `migrations/`, shipped beside the compiled output. */
const LEDGER_DIR = join(__dirname, 'migrations');

async function appliedIds(client: ClickHouseClient): Promise<Set<string>> {
  const result = await client.query({ query: `SELECT id FROM ${LEDGER}`, format: 'JSONEachRow' });
  const rows = await result.json<{ id: string }>();
  return new Set(rows.map((r) => r.id));
}

/**
 * Creates the ledger, which is the one migration that cannot be recorded.
 *
 * It has to exist before the runner can ask what is pending, so it is applied
 * unconditionally every time. `CREATE TABLE IF NOT EXISTS` is what makes that
 * a no-op on the second run. It is still a `.sql` file like every other
 * migration, so the whole schema is readable in one place and by one means.
 */
async function bootstrapLedger(client: ClickHouseClient): Promise<void> {
  const [ledger] = await loadMigrations([LEDGER_DIR]);
  const [statement] = ledger?.statements ?? [];
  if (!statement) throw new Error(`no ledger migration found in ${LEDGER_DIR}`);
  await client.command({ query: statement });
}

/**
 * Brings the schema up to date, and is safe to run twice.
 *
 * Takes the migration set rather than discovering it: packages own the tables
 * they define, and the application is what knows which of them are deployed
 * together. {@link ordered} then rejects a set whose ordinals collide, so the
 * order is a property of the filenames rather than of the caller's array.
 *
 * Deliberately not called at application boot. Two replicas starting together
 * would race each other through the same DDL, so this runs as its own step: a
 * one-shot service in compose, an explicit command before a deploy.
 */
export async function migrate(
  client: ClickHouseClient,
  migrations: readonly Migration[],
): Promise<string[]> {
  const pending = ordered(migrations);

  await bootstrapLedger(client);
  const done = await appliedIds(client);
  const applied: string[] = [];

  for (const migration of pending) {
    if (done.has(migration.id)) continue;

    for (const statement of migration.statements) {
      // Sequential because migrations are order-dependent: a view selects from
      // the table created by the migration before it, and within a file the
      // same holds statement to statement.
      // oxlint-disable-next-line no-await-in-loop
      await client.command({ query: statement });
    }
    // Recorded only once the whole file lands, so a failure part-way through is
    // retried from its first statement rather than skipped. That is safe
    // because every statement is `IF NOT EXISTS`, and it is why they have to be.
    // oxlint-disable-next-line no-await-in-loop
    await client.insert({ table: LEDGER, values: [{ id: migration.id }], format: 'JSONEachRow' });
    applied.push(migration.id);
  }

  return applied;
}
