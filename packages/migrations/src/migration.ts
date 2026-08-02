import { readdir, readFile } from 'node:fs/promises';
import { join } from 'node:path';

/**
 * One `.sql` file, and the statements in it.
 *
 * Migrations are SQL on disk rather than strings in TypeScript so that the
 * schema reads as schema: reviewable as a diff, runnable by hand against a
 * server when something needs checking, and not something a refactor of the
 * surrounding code can quietly alter. A file may hold several statements, which
 * is why this is a list.
 *
 * They deliberately do not all live in one package. A package that defines a
 * table keeps that table's migration beside it, and the runner is handed the
 * union — so adding a table is one package's business, and this package never
 * becomes a catalogue of every table in the system.
 */
export interface Migration {
  /** The file's basename without `.sql`: `NNN_snake_case`. */
  readonly id: string;
  /** In file order. Usually one; see {@link STATEMENT_SEPARATOR}. */
  readonly statements: readonly string[];
}

/**
 * Separates statements within one migration file.
 *
 * ClickHouse's HTTP interface refuses multi-statement requests outright —
 * `Multi-statements are not allowed` — so a file holding more than one has to be
 * split before it is sent. **Not on `;`.** These files carry semicolons inside
 * comments and inside string literals (`CAST(NULL, 'Nullable(UInt8)')` sits next
 * to prose that uses them), so splitting on the character means writing a SQL
 * lexer that knows about quoting, escapes and both comment forms — one nobody
 * would think to test until it silently truncated a migration.
 *
 * A marker the author writes instead needs no lexer, cannot collide with SQL
 * content, and is itself a comment, so the file stays valid SQL. A statement
 * still belongs in its own file unless it is meaningless apart from its
 * neighbours: the nine projections of one table are one change to read and one
 * change to review, where a table and the view over it are two.
 */
export const STATEMENT_SEPARATOR = /^[ \t]*--@statement[ \t]*$/m;

/**
 * Splits on {@link STATEMENT_SEPARATOR}, dropping sections that hold no SQL.
 *
 * A file's header sits before its first marker and would otherwise be sent on
 * its own, which ClickHouse rejects as an empty query. "Holds SQL" is one line
 * that is neither blank nor a comment — enough here without a lexer, because a
 * section where *every* non-blank line opens with `--` cannot also contain a
 * statement.
 */
export function splitStatements(sql: string): string[] {
  return sql
    .split(STATEMENT_SEPARATOR)
    .map((statement) => statement.trim())
    .filter((statement) =>
      statement
        .split('\n')
        .some((line) => line.trim() !== '' && !line.trimStart().startsWith('--')),
    );
}

const ID_PATTERN = /^(\d{3})_[a-z0-9_]+$/;

/**
 * Rejects a migration set that cannot be ordered unambiguously.
 *
 * The ordinal has to be unique across *every* contributing package, not just
 * within one directory. Two packages that each start at `001` would otherwise
 * apply in whatever order the caller happened to concatenate them, which is a
 * schema that depends on an import order — reproducible right up until someone
 * reorders the list. Failing loudly, naming both sides, is cheap; discovering
 * it from a diverged database is not.
 */
export function assertOrderable(migrations: readonly Migration[]): void {
  const byOrdinal = new Map<string, string>();

  for (const { id } of migrations) {
    const match = ID_PATTERN.exec(id);
    if (!match) {
      throw new Error(
        `migration "${id}" must be named NNN_snake_case.sql, e.g. 001_spoke_events.sql`,
      );
    }

    const ordinal = match[1]!;
    const clash = byOrdinal.get(ordinal);
    if (clash !== undefined) {
      throw new Error(`migration ordinal ${ordinal} is claimed by both "${clash}" and "${id}"`);
    }
    byOrdinal.set(ordinal, id);
  }
}

/** Orders a migration set by ordinal, having checked it can be. */
export function ordered(migrations: readonly Migration[]): Migration[] {
  assertOrderable(migrations);
  return migrations.toSorted((a, b) => a.id.localeCompare(b.id));
}

/**
 * Reads every `.sql` file in the given directories.
 *
 * A file is one migration and is recorded once, however many statements it
 * holds — so a file that fails halfway is retried from the top, and every
 * statement in one is written `IF NOT EXISTS` for that reason.
 *
 * Ordering is by filename across all directories at once, so a package's
 * migrations interleave with another's by ordinal rather than being grouped by
 * whichever directory was listed first.
 */
export async function loadMigrations(directories: readonly string[]): Promise<Migration[]> {
  const found = await Promise.all(directories.map(readDirectory));
  return ordered(found.flat());
}

async function readDirectory(directory: string): Promise<Migration[]> {
  const entries = await readdir(directory);
  const files = entries.filter((entry) => entry.endsWith('.sql'));

  return Promise.all(
    files.map(async (file) => ({
      id: file.slice(0, -'.sql'.length),
      statements: splitStatements(await readFile(join(directory, file), 'utf8')),
    })),
  );
}
