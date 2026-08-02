import { join } from 'node:path';

/**
 * The Postgres tables this package owns, shipped beside the compiled output.
 *
 * A directory rather than the loaded migrations, for the reason
 * `POSITION_MIGRATIONS_DIR` gives: reading it is filesystem work, and importing
 * this package to run a query should not do any.
 *
 * **A second migration set in one package, and the engine is what separates
 * them.** Ordinals only have to be unique within a database, so this one starts
 * at `010` to clear the indexing package's `001`/`002` and has nothing to say
 * about the ClickHouse set's own `010`. The application keeps two lists for
 * exactly that reason and never concatenates them.
 *
 * That this package now writes to both databases is not drift. The fold lives
 * in ClickHouse because it is an aggregate over an event log; token metadata
 * lives in Postgres because it is a small keyed dimension that gets replaced in
 * place — see `010_token_metadata.sql` for the measurements behind that.
 */
export const POSITION_POSTGRES_MIGRATIONS_DIR = join(__dirname, 'postgres-migrations');
