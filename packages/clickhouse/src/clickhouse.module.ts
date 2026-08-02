import { createClient, type ClickHouseClient } from '@clickhouse/client';
import { trace } from '@opentelemetry/api';
import {
  Module,
  type DynamicModule,
  type InjectionToken,
  type ModuleMetadata,
  type OptionalFactoryDependency,
} from '@nestjs/common';

import { ClickHouseHealthIndicator } from './clickhouse.health-indicator';
import {
  CLICKHOUSE_CLIENT,
  CLICKHOUSE_OPTIONS,
  type ClickHouseOptions,
} from './clickhouse.options';

export interface ClickHouseAsyncOptions<TDeps extends unknown[] = unknown[]> extends Pick<
  ModuleMetadata,
  'imports'
> {
  inject?: (InjectionToken | OptionalFactoryDependency)[];
  useFactory: (...args: TDeps) => ClickHouseOptions | Promise<ClickHouseOptions>;
  /**
   * Export the client to importing modules as well as to this one.
   *
   * `false` — the default — keeps it private, which is what a service wants
   * when only repositories declared here should be able to reach the database.
   */
  global?: boolean;
}

/**
 * Provides the ClickHouse client, and nothing that knows what is stored in it.
 *
 * Repositories live with whatever owns their tables and inject
 * {@link CLICKHOUSE_CLIENT} from here, so this module never grows a list of
 * every table in the system. Import it wherever such a repository is declared.
 */
@Module({})
export class ClickHouseModule {
  static forRootAsync<TDeps extends unknown[]>(
    options: ClickHouseAsyncOptions<TDeps>,
  ): DynamicModule {
    return {
      module: ClickHouseModule,
      global: options.global ?? false,
      imports: options.imports ?? [],
      providers: [
        {
          provide: CLICKHOUSE_OPTIONS,
          useFactory: options.useFactory,
          inject: options.inject,
        },
        {
          provide: CLICKHOUSE_CLIENT,
          // `createClient` opens no socket, so boot does not depend on the
          // database being up — the pod reports not-ready instead of
          // crash-looping, matching how the chain client behaves.
          useFactory: (config: ClickHouseOptions): ClickHouseClient =>
            createClient({
              url: config.url,
              database: config.database,
              username: config.username,
              password: config.password,
              // The client declares `ClickHouseTracer` as a structural subset
              // of the OpenTelemetry `Tracer`, so a real tracer is assignable
              // with no adapter — which is why there is no wrapper here, unlike
              // Postgres. It sets the semantic-convention attributes itself
              // (`db.system.name`, `db.collection.name`, and the whole
              // `X-ClickHouse-Summary`, including `written_rows`), and the
              // `node:http` span from the HTTP instrumentation nests beneath.
              //
              // Two things the vendor is explicit about and that matter here.
              // These calls are inlined into the hot path with **no defensive
              // wrapper**, so a throwing tracer would surface as a failed
              // query — one of the few places telemetry can break the
              // application. And `query` emits two spans: `clickhouse.query`
              // ends at the response headers, while `clickhouse.query.stream`
              // is owned by the `ResultSet` and ends only when it is consumed.
              // Every call site here drains its result, which is now an
              // invariant rather than a habit.
              tracer: trace.getTracer('@packages/clickhouse'),
            }),
          inject: [CLICKHOUSE_OPTIONS],
        },
        ClickHouseHealthIndicator,
      ],
      exports: [CLICKHOUSE_CLIENT, CLICKHOUSE_OPTIONS, ClickHouseHealthIndicator],
    };
  }
}
