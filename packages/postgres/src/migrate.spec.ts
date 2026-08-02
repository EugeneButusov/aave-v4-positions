import type { Migration } from '@packages/migrations';
import postgres from 'postgres';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import { migrate } from './migrate';

const URL = process.env['POSTGRES_URL'] ?? '';

/**
 * A schema of its own, because these specs drop `schema_migrations` between
 * tests and assert on the exact set of tables that exist. Vitest runs projects
 * concurrently against one server, so in `public` this would be fighting the
 * indexing adapters' specs — each wiping the other's ledger, and both
 * intermittently.
 */
const SCHEMA = 'migrate_spec';

/**
 * A real server, not a fake. What these assert is that the DDL executes and
 * that a failed run rolls back — the second is the whole reason this runner
 * differs from the ClickHouse one, and no double can tell you either.
 */
const sql = postgres(URL, { max: 2, connection: { search_path: SCHEMA } });

beforeAll(async () => {
  // Its own client: `search_path` is a startup parameter, so the connection
  // above cannot be the one that creates the schema it points at.
  const admin = postgres(URL, { max: 1 });
  try {
    await admin`CREATE SCHEMA IF NOT EXISTS ${admin(SCHEMA)}`;
  } finally {
    await admin.end();
  }
});

function at(id: string, ...statements: string[]): Migration {
  return { id, statements };
}

const WIDGETS = at('001_widgets', 'CREATE TABLE widgets (id text PRIMARY KEY)');
const GADGETS = at('002_gadgets', 'CREATE TABLE gadgets (id text PRIMARY KEY)');
const BROKEN = at('002_broken', 'CREATE TABLE gadgets (id text PRIMARY KEY, ');

async function tables(): Promise<string[]> {
  const rows = await sql<{ table_name: string }[]>`
    SELECT table_name FROM information_schema.tables WHERE table_schema = ${SCHEMA}
  `;
  return rows.map((row) => row.table_name).toSorted();
}

async function recorded(): Promise<string[]> {
  const rows = await sql<{ id: string }[]>`SELECT id FROM schema_migrations ORDER BY id`;
  return rows.map((row) => row.id);
}

beforeEach(async () => {
  await sql`DROP TABLE IF EXISTS widgets, gadgets, schema_migrations`;
});

afterAll(async () => {
  await sql.end();
});

describe('migrate', () => {
  it('creates the ledger even when there is nothing to apply', async () => {
    await expect(migrate(sql, [])).resolves.toEqual([]);

    // The ledger is the one migration that cannot record itself, so it is
    // applied unconditionally rather than being pending.
    expect(await tables()).toEqual(['schema_migrations']);
  });

  it('applies each migration in ordinal order and records it', async () => {
    // Handed over backwards on purpose: the order is a property of the
    // filenames, not of how the application concatenated its directories.
    await expect(migrate(sql, [GADGETS, WIDGETS])).resolves.toEqual(['001_widgets', '002_gadgets']);

    expect(await tables()).toEqual(['gadgets', 'schema_migrations', 'widgets']);
    expect(await recorded()).toEqual(['001_widgets', '002_gadgets']);
  });

  it('applies nothing on a second run', async () => {
    await migrate(sql, [WIDGETS, GADGETS]);

    await expect(migrate(sql, [WIDGETS, GADGETS])).resolves.toEqual([]);
    expect(await recorded()).toEqual(['001_widgets', '002_gadgets']);
  });

  it('applies every statement in a multi-statement file', async () => {
    const both = at(
      '001_widgets',
      'CREATE TABLE widgets (id text PRIMARY KEY)',
      'CREATE TABLE gadgets (id text PRIMARY KEY)',
    );

    // One file, one ledger row, however many statements — so a file that fails
    // part-way is retried from its first statement, which is why every one of
    // them is written `IF NOT EXISTS`.
    await expect(migrate(sql, [both])).resolves.toEqual(['001_widgets']);

    expect(await tables()).toEqual(['gadgets', 'schema_migrations', 'widgets']);
    expect(await recorded()).toEqual(['001_widgets']);
  });

  it('leaves nothing behind when a migration fails', async () => {
    await migrate(sql, [WIDGETS]);

    // Asserted on the server's own message, so the test fails loudly if the
    // migration starts failing for some reason other than the one it plants.
    await expect(migrate(sql, [WIDGETS, BROKEN])).rejects.toThrow(/syntax error/);

    // The property the ClickHouse runner cannot offer: DDL is transactional
    // here, so a failed run is not a half-migrated schema somebody has to clean
    // up by hand before retrying. What committed earlier stays; this run left
    // no table and no ledger row.
    expect(await tables()).toEqual(['schema_migrations', 'widgets']);
    expect(await recorded()).toEqual(['001_widgets']);
  });

  it('refuses a set whose ordinals collide, before touching the database', async () => {
    await expect(migrate(sql, [WIDGETS, GADGETS, BROKEN])).rejects.toThrow(
      /ordinal 002 is claimed by both/,
    );

    // Rejected by `ordered` ahead of the transaction, so not even the ledger
    // was created.
    expect(await tables()).toEqual([]);
  });
});
