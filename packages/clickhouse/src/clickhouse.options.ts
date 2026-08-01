export interface ClickHouseOptions {
  readonly url: string;
  readonly database: string;
  readonly username: string;
  readonly password: string;
}

export const CLICKHOUSE_OPTIONS = Symbol('CLICKHOUSE_OPTIONS');

/** The `@clickhouse/client` instance, shared by the store and the health probe. */
export const CLICKHOUSE_CLIENT = Symbol('CLICKHOUSE_CLIENT');
