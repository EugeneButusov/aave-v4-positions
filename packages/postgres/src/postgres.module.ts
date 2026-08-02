import {
  Logger,
  Module,
  type DynamicModule,
  type InjectionToken,
  type ModuleMetadata,
  type OptionalFactoryDependency,
} from '@nestjs/common';
import postgres, { type Sql } from 'postgres';

import { PostgresHealthIndicator } from './postgres.health-indicator';
import { POSTGRES_CLIENT, POSTGRES_OPTIONS, type PostgresOptions } from './postgres.options';
import { tracedSql } from './traced-sql';

/**
 * Small on purpose. The indexing loop is strictly sequential — one iteration
 * awaits the last, because overlapping two would race the cursor — so nothing
 * here needs concurrency for throughput. The pool exists so a readiness
 * `SELECT 1` does not queue behind an in-flight cursor save and time the probe
 * out, which a single connection would guarantee under load.
 */
const DEFAULT_MAX_CONNECTIONS = 4;

const logger = new Logger('Postgres');

export interface PostgresAsyncOptions<TDeps extends unknown[] = unknown[]> extends Pick<
  ModuleMetadata,
  'imports'
> {
  inject?: (InjectionToken | OptionalFactoryDependency)[];
  useFactory: (...args: TDeps) => PostgresOptions | Promise<PostgresOptions>;
  /**
   * Export the client to importing modules as well as to this one.
   *
   * `false` — the default — keeps it private, which is what a service wants
   * when only the stores declared here should be able to reach the database.
   */
  global?: boolean;
}

/**
 * Provides the Postgres client, and nothing that knows what is stored in it.
 *
 * Stores live with whatever owns their tables and inject {@link POSTGRES_CLIENT}
 * from here, so this module never grows a list of every table in the system.
 * Import it wherever such a store is declared.
 */
@Module({})
export class PostgresModule {
  static forRootAsync<TDeps extends unknown[]>(
    options: PostgresAsyncOptions<TDeps>,
  ): DynamicModule {
    return {
      module: PostgresModule,
      global: options.global ?? false,
      imports: options.imports ?? [],
      providers: [
        {
          provide: POSTGRES_OPTIONS,
          useFactory: options.useFactory,
          inject: options.inject,
        },
        {
          provide: POSTGRES_CLIENT,
          // postgres.js connects lazily, on the first query, so boot does not
          // depend on the database being up — the pod reports not-ready instead
          // of crash-looping, matching ClickHouse and the chain client.
          //
          // Wrapped for tracing here rather than in each store: five adapters
          // inject this client and issue nine queries between them, and this is
          // the one place all of them pass through. With no SDK registered the
          // wrapper's spans are the API's no-ops.
          useFactory: (config: PostgresOptions): Sql =>
            tracedSql(
              postgres(config.url, {
                max: config.maxConnections ?? DEFAULT_MAX_CONNECTIONS,
                // Left to itself, postgres.js writes server notices straight to
                // stdout as an inspected object — several lines of it, none of
                // them JSON. The deployed log contract is one JSON object per
                // line, so a single notice would break every parser reading the
                // stream. Debug level because a notice reaching a running indexer
                // is informational by definition; anything that matters arrives
                // as an error.
                onnotice: (notice) => logger.debug(notice['message'] ?? JSON.stringify(notice)),
                // `prepare` is left at its default of `true`, and named here
                // only so the next person does not have to rediscover it:
                // behind PgBouncer in transaction-pooling mode it has to be
                // `false`, or every query after the first fails with "prepared
                // statement already exists" — an error that does not name its
                // own cause.
              }),
            ),
          inject: [POSTGRES_OPTIONS],
        },
        PostgresHealthIndicator,
      ],
      exports: [POSTGRES_CLIENT, POSTGRES_OPTIONS, PostgresHealthIndicator],
    };
  }
}
