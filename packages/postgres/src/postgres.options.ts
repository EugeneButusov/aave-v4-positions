export interface PostgresOptions {
  /**
   * A libpq connection URL: `postgres://user:password@host:5432/database`.
   *
   * One URL rather than the discrete fields the ClickHouse options carry. Every
   * managed Postgres hands you exactly this string, often with an `?sslmode=`
   * parameter attached, and splitting it into parts means reassembling it —
   * where the first thing reassembly gets wrong is percent-encoding a password.
   */
  readonly url: string;

  /**
   * Pool size. Defaults to something small on purpose; see the module.
   */
  readonly maxConnections?: number;
}

export const POSTGRES_OPTIONS = Symbol('POSTGRES_OPTIONS');

/** The postgres.js instance, shared by the stores and the health probe. */
export const POSTGRES_CLIENT = Symbol('POSTGRES_CLIENT');
