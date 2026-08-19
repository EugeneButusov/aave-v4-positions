import { readdir, readFile } from 'node:fs/promises';
import { basename, join } from 'node:path';

import type { ClickHouseClient } from '@clickhouse/client';

/**
 * Applies every `.sql` file in these directories to a database a spec just made.
 *
 * **Not a migration runner.** `bins/migrate` is the only one, and the only thing
 * that keeps a ledger or decides what is pending. A spec knows its database is
 * empty, so there is nothing to skip.
 *
 * Ordering is by filename across all the directories at once rather than
 * directory by directory: a projection reads a table another package creates,
 * and `010 > 002` is the entire guarantee that it is created first.
 */
export async function applySql(
  client: ClickHouseClient,
  directories: readonly string[],
): Promise<void> {
  const found = await Promise.all(
    directories.map(async (directory) =>
      (await readdir(directory))
        .filter((entry) => entry.endsWith('.sql'))
        .map((entry) => join(directory, entry)),
    ),
  );

  for (const file of found.flat().toSorted((a, b) => basename(a).localeCompare(basename(b)))) {
    // oxlint-disable-next-line no-await-in-loop
    await client.command({ query: await readFile(file, 'utf8') });
  }
}
