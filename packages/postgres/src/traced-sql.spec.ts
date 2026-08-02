import { SpanStatusCode, trace } from '@opentelemetry/api';
import {
  BasicTracerProvider,
  InMemorySpanExporter,
  SimpleSpanProcessor,
  type ReadableSpan,
} from '@opentelemetry/sdk-trace-base';
import postgres, { type Sql } from 'postgres';
import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest';

import { tracedSql } from './traced-sql';

const URL = process.env['POSTGRES_URL'] ?? 'postgres://postgres@localhost:5432/postgres';
const SCHEMA = 'traced_sql_spec';

const exporter = new InMemorySpanExporter();

let admin: Sql;
let sql: Sql;

/** Only the spans this wrapper made; the SDK is shared with nothing else here. */
function spans(): ReadableSpan[] {
  return exporter.getFinishedSpans();
}

beforeAll(async () => {
  const provider = new BasicTracerProvider({
    spanProcessors: [new SimpleSpanProcessor(exporter)],
  });
  trace.setGlobalTracerProvider(provider);

  admin = postgres(URL, { max: 1 });
  await admin`CREATE SCHEMA IF NOT EXISTS ${admin(SCHEMA)}`;

  // `search_path` is a startup parameter, so it has to be set on connect —
  // the same shape the cursor-store spec uses.
  sql = tracedSql(postgres(URL, { max: 2, connection: { search_path: SCHEMA } }));
  await sql`CREATE TABLE IF NOT EXISTS labels (id int PRIMARY KEY, name text)`;
  exporter.reset();
});

afterEach(() => {
  exporter.reset();
});

afterAll(async () => {
  await sql.end();
  await admin`DROP SCHEMA IF EXISTS ${admin(SCHEMA)} CASCADE`;
  await admin.end();
  trace.disable();
});

describe('tracedSql', () => {
  it('records one span per executed query, named for the operation and table', async () => {
    await sql`INSERT INTO labels (id, name) VALUES (${1}, ${'usdc'})`;

    expect(spans()).toHaveLength(1);
    const [span] = spans();
    expect(span?.name).toBe('INSERT labels');
    expect(span?.attributes['db.system.name']).toBe('postgresql');
    expect(span?.attributes['db.operation.name']).toBe('INSERT');
  });

  it('elides interpolated values from db.query.text', async () => {
    await sql`SELECT name FROM labels WHERE name = ${'usdc'}`;

    // The guard that keeps a parameter out of a span attribute. `strings.raw`
    // is the template's literal halves, so the value cannot be in there — a
    // wrapper built from the *interpolated* string would ship it to the
    // backend.
    const text = spans()[0]?.attributes['db.query.text'];
    expect(text).toBe('SELECT name FROM labels WHERE name = ?');
    expect(text).not.toContain('usdc');
  });

  it('does not take a table name out of a comment', async () => {
    // Not hypothetical, and not invented for the spec: this is the wording in
    // `postgres-sync-status-store.ts`, and against the raw text the table
    // pattern matched `from its` and named the span `SELECT its`. Found by
    // reading a real trace out of Tempo.
    await sql`
      SELECT id
      -- \`updated_at\` is the database's own now(), so a reader subtracting it
      -- from its own clock reports skew as staleness.
      FROM labels
      WHERE id = ${1}
    `;

    expect(spans()[0]?.name).toBe('SELECT labels');
  });

  it('collapses a multi-line statement into one readable line', async () => {
    await sql`
      SELECT
          name
      FROM labels
    `;

    expect(spans()[0]?.attributes['db.query.text']).toBe('SELECT name FROM labels');
  });

  it('leaves the query lazy: building one starts no span, awaiting it does', async () => {
    const pending = sql`SELECT 1 AS one`;
    expect(spans()).toHaveLength(0);

    await pending;
    expect(spans()).toHaveLength(1);
  });

  it('executes the query exactly once despite the span also subscribing', async () => {
    await sql`INSERT INTO labels (id, name) VALUES (${2}, ${'dai'})`;
    const [row] = await sql`SELECT count(*)::int AS n FROM labels WHERE id = ${2}`;

    // The whole reason the wrapper can subscribe at all: postgres.js guards
    // execution behind an `executed` flag, so a second `then` is free. If that
    // ever changed, this row count would read 2.
    expect(row?.['n']).toBe(1);
  });

  it('marks the span an error and still rejects for the caller', async () => {
    await expect(sql`SELECT * FROM table_that_does_not_exist`).rejects.toThrow(
      /table_that_does_not_exist/,
    );

    const [span] = spans();
    expect(span?.status.code).toBe(SpanStatusCode.ERROR);
    expect(span?.events.some((event) => event.name === 'exception')).toBe(true);
  });

  describe('the forms that are not queries', () => {
    it('opens no span for the identifier helper', async () => {
      // `sql('labels')` builds an identifier and is called *inside* a template.
      // Tracing it would emit a span for a fragment that never reaches the
      // server — and `postgres-token-metadata-store.ts` uses exactly this form.
      await sql`SELECT name FROM ${sql('labels')} WHERE id = ${1}`;

      expect(spans()).toHaveLength(1);
      expect(spans()[0]?.attributes['db.operation.name']).toBe('SELECT');
    });

    it('opens no span for the insert-values helper', async () => {
      const values = [{ id: 3, name: 'weth' }];
      await sql`INSERT INTO labels ${sql(values, 'id', 'name')}`;

      expect(spans()).toHaveLength(1);
      expect(spans()[0]?.attributes['db.operation.name']).toBe('INSERT');
    });
  });

  describe('transparency', () => {
    it('passes through the members the migration runner uses', async () => {
      expect(typeof sql.unsafe).toBe('function');
      expect(typeof sql.begin).toBe('function');
      expect(typeof sql.file).toBe('function');
      expect(typeof sql.end).toBe('function');

      const [unsafe] = await sql.unsafe('SELECT 4 AS four');
      expect(unsafe?.['four']).toBe(4);

      const inTransaction = await sql.begin(async (tx) => {
        const [row] = await tx<{ five: number }[]>`SELECT 5 AS five`;
        return row?.five;
      });
      expect(inTransaction).toBe(5);
    });
  });
});
