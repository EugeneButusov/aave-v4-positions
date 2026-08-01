import { readdir, readFile } from 'node:fs/promises';
import { join } from 'node:path';

/**
 * One `.sql` file, and the one statement in it.
 *
 * Migrations are SQL on disk rather than strings in TypeScript so that the
 * schema reads as schema: reviewable as a diff, runnable by hand against a
 * server when something needs checking, and not something a refactor of the
 * surrounding code can quietly alter.
 *
 * They deliberately do not all live in one package. A package that defines a
 * table keeps that table's migration beside it, and the runner is handed the
 * union — so adding a table is one package's business, and this package never
 * becomes a catalogue of every table in the system.
 */
export interface Migration {
  /** The file's basename without `.sql`: `NNN_snake_case`. */
  readonly id: string;
  /** Exactly one statement. See {@link loadMigrations}. */
  readonly sql: string;
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
 * **One statement per file.** ClickHouse's HTTP interface accepts a single
 * statement per request, and splitting a file on `;` means writing a SQL parser
 * that has to know about semicolons inside string literals and comments — a
 * parser nobody would think to test until it silently truncated a migration. A
 * table and the view over it are therefore two files, which also means each is
 * recorded and retried independently.
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
      sql: await readFile(join(directory, file), 'utf8'),
    })),
  );
}
