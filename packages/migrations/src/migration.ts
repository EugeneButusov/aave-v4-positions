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
  /** In file order. Usually one; see {@link splitStatements}. */
  readonly statements: readonly string[];
}

/** Where the scan is when it meets a character. Only `code` can end a statement. */
type Lexical = 'code' | 'comment' | "'" | '"' | '`';

/**
 * Splits a migration file into the statements it holds, on `;`.
 *
 * ClickHouse's HTTP interface refuses multi-statement requests outright —
 * `Multi-statements are not allowed` — so a file holding more than one has to be
 * taken apart before it is sent. Postgres would accept them, but is split the
 * same way so that one rule covers both.
 *
 * **The terminator is `;` because these files are written to be pasted.** A
 * migration you cannot drop into a SQL console and run is one you cannot debug
 * when it matters — and a file that separates statements with a comment marker
 * cannot be: the console parses the whole buffer as a single query and fails at
 * the second statement, having created nothing.
 *
 * Which is why this cannot simply cut on the character. Measured over the twenty
 * files here: 723 `--` comments, 184 string literals, 215 backtick identifiers
 * and 14 double-quoted ones — with nineteen semicolons living inside that prose,
 * across half the files. So the scan tracks what it is inside, and only a
 * semicolon in open code ends a statement.
 *
 * Deliberately not handled, because the corpus contains none of them and untested
 * code is worse than absent code: block comments, `''` and backslash escapes, and
 * dollar quoting. A test holds the corpus to that.
 */
export function splitStatements(sql: string): string[] {
  const statements: string[] = [];
  let start = 0;
  let inside: Lexical = 'code';

  for (let index = 0; index < sql.length; index++) {
    const char = sql[index]!;

    if (inside === 'code') {
      if (char === '-' && sql[index + 1] === '-') inside = 'comment';
      else if (char === "'" || char === '"' || char === '`') inside = char;
      else if (char === ';') {
        pushIfSql(statements, sql.slice(start, index));
        start = index + 1;
      }
    } else if (inside === 'comment') {
      if (char === '\n') inside = 'code';
    } else if (char === inside) {
      inside = 'code';
    }
  }

  // Whatever follows the last `;`: a trailing comment in a well-formed file, and
  // an unterminated statement otherwise. Dropping the latter silently is how a
  // table goes missing at deploy time.
  pushIfSql(statements, sql.slice(start));

  return statements;
}

/**
 * Keeps a section only if it holds one line that is neither blank nor a comment.
 *
 * A file opens with prose, so the text before its first `;` is a comment block
 * that would otherwise be sent on its own — which ClickHouse rejects as an empty
 * query, from the migration runner, at deploy time.
 */
function pushIfSql(statements: string[], section: string): void {
  const statement = section.trim();
  if (statement.split('\n').some((line) => line.trim() !== '' && !line.trim().startsWith('--'))) {
    statements.push(statement);
  }
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
