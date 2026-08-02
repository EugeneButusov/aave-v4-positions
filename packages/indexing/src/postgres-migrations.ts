import { join } from 'node:path';

/**
 * The tables the Postgres adapters own, shipped beside the compiled output.
 *
 * A directory rather than the loaded migrations: reading it is filesystem work,
 * and importing this package to write a processor should not do any. The
 * application hands it to the runner as its own step.
 *
 * The `.sql` files live at the package root rather than in `cursor/` or
 * `reorg/`, because the runner takes directories and the two tables deploy as
 * one set — splitting them would mean two directories whose ordinals still have
 * to be unique across both.
 *
 * **The engine is in the directory name**, because nothing else here says it.
 * A migration set is only valid against the database it was written for, the
 * two runners take the same `NNN_snake_case.sql` shape, and `@packages/indexing`
 * — unlike `@packages/postgres` — is not a name that tells you which one. The
 * ordinal has to lead the *filename*, so this is the only place the label fits.
 */
export const INDEXING_MIGRATIONS_DIR = join(__dirname, 'postgres-migrations');
