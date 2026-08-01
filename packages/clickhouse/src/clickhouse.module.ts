import { createClient, type ClickHouseClient } from '@clickhouse/client';
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
            }),
          inject: [CLICKHOUSE_OPTIONS],
        },
        ClickHouseHealthIndicator,
      ],
      exports: [CLICKHOUSE_CLIENT, CLICKHOUSE_OPTIONS, ClickHouseHealthIndicator],
    };
  }
}
