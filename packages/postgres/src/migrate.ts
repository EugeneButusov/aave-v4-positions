import { join } from 'node:path';

import { loadMigrations, ordered, type Migration } from '@packages/migrations';
import type { Sql, TransactionSql } from 'postgres';

/** This package's own `migrations/`, shipped beside the compiled output. */
const LEDGER_DIR = join(__dirname, 'migrations');

/**
 * Arbitrary, but fixed forever: it only has to be a number no other advisory
 * lock in this database claims. Changing it would let an old deployment's
 * migrate step run alongside a new one, which is the single thing the lock is
 * here to prevent.
 */
const MIGRATION_LOCK_KEY = 4_017_460_985;

async function appliedIds(tx: TransactionSql): Promise<Set<string>> {
  const rows = await tx<{ id: string }[]>`SELECT id FROM schema_migrations`;
  return new Set(rows.map((row) => row.id));
}

/**
 * Brings the schema up to date, and is safe to run twice.
 *
 * Takes the migration set rather than discovering it: packages own the tables
 * they define, and the application is what knows which of them are deployed
 * together. {@link ordered} then rejects a set whose ordinals collide, so the
 * order is a property of the filenames rather than of the caller's array.
 *
 * **The whole run is one transaction**, which is where this differs from the
 * ClickHouse runner and why it is allowed to. Postgres DDL is transactional, so
 * a set that fails partway leaves nothing behind — no half-created table to
 * drop by hand before the retry, and no ledger row for a migration that did not
 * finish. The cost is that a long set holds one transaction open; with a handful
 * of small DDL statements that is not a trade worth reversing.
 *
 * The advisory lock is transaction-scoped for the same reason: it is released by
 * the commit or the rollback, so a runner that dies cannot leave the next one
 * waiting on a lock nobody holds. Compose already serialises this with a
 * one-shot service; a deploy pipeline that runs it from two jobs would not.
 *
 * Deliberately not called at application boot. Two replicas starting together
 * would race each other through the same DDL, so this runs as its own step.
 */
export async function migrate(sql: Sql, migrations: readonly Migration[]): Promise<string[]> {
  const pending = ordered(migrations);

  const [ledger] = await loadMigrations([LEDGER_DIR]);
  const [ledgerStatement] = ledger?.statements ?? [];
  if (!ledgerStatement) throw new Error(`no ledger migration found in ${LEDGER_DIR}`);

  return sql.begin(async (tx) => {
    await tx`SELECT pg_advisory_xact_lock(${MIGRATION_LOCK_KEY})`;

    // `unsafe` because DDL cannot be parameterised, and there is nothing unsafe
    // about it here: the string is a `.sql` file from this repository, read off
    // disk at deploy time, never anything a request supplied.
    await tx.unsafe(ledgerStatement);

    const done = await appliedIds(tx);
    const applied: string[] = [];

    for (const migration of pending) {
      if (done.has(migration.id)) continue;

      for (const statement of migration.statements) {
        // Sequential because migrations are order-dependent: a view selects from
        // the table created by the migration before it, and within a file the
        // same holds statement to statement.
        // oxlint-disable-next-line no-await-in-loop
        await tx.unsafe(statement);
      }

      // oxlint-disable-next-line no-await-in-loop
      await tx`INSERT INTO schema_migrations ${tx({ id: migration.id })} ON CONFLICT (id) DO NOTHING`;
      applied.push(migration.id);
    }

    return applied;
  });
}
