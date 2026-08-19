import { readdir, readFile } from 'node:fs/promises';
import { basename, join } from 'node:path';

import type { Sql } from 'postgres';

/**
 * Applies every `.sql` file in these directories to a schema a spec just made.
 *
 * **Not a migration runner.** `bins/migrate` is the only one, and the only thing
 * that keeps a ledger or decides what is pending. A spec creates its schema a
 * moment before calling this, so there is nothing to skip — and every migration
 * in the repository is written `IF NOT EXISTS`, which is what makes a second
 * call a no-op without anything recording the first.
 *
 * Ordering is by filename across all the directories at once rather than
 * directory by directory: a projection reads a table another package creates,
 * and `010 > 002` is the entire guarantee that it is created first.
 */
export async function applySql(sql: Sql, directories: readonly string[]): Promise<void> {
  const found = await Promise.all(
    directories.map(async (directory) =>
      (await readdir(directory))
        .filter((entry) => entry.endsWith('.sql'))
        .map((entry) => join(directory, entry)),
    ),
  );

  for (const file of found.flat().toSorted((a, b) => basename(a).localeCompare(basename(b)))) {
    // `unsafe` because DDL cannot be parameterised, and there is nothing unsafe
    // about it: the string is a file from this repository, not a request.
    // oxlint-disable-next-line no-await-in-loop
    await sql.unsafe(await readFile(file, 'utf8'));
  }
}
