import {
  Module,
  type DynamicModule,
  type InjectionToken,
  type ModuleMetadata,
  type OptionalFactoryDependency,
} from '@nestjs/common';
import { ClickHouseModule, type ClickHouseOptions } from '@packages/clickhouse';
import { PostgresModule, type PostgresOptions } from '@packages/postgres';

import { ClickHouseHubAssetStore } from './store/clickhouse-hub-asset-store';
import { ClickHousePositionStore } from './store/clickhouse-position-store';
import { HUB_ASSET_STORE } from './store/hub-asset-store';
import { POSITION_STORE } from './store/position-store';
import { PostgresTokenMetadataStore } from './store/postgres-token-metadata-store';
import { ClickHouseTokenListings, TOKEN_LISTINGS } from './store/token-listing-source';
import { TOKEN_METADATA_STORE } from './store/token-metadata-store';

const POSITIONS_OPTIONS = Symbol('POSITIONS_OPTIONS');

export interface PositionsOptions {
  readonly clickhouse: ClickHouseOptions;
  /**
   * Where token metadata lives.
   *
   * **Two databases in one module, and they hold different kinds of thing.**
   * ClickHouse holds the folds — aggregates a materialized view maintains over
   * an event log. Postgres holds the one table here that is fetched rather than
   * folded, replaced in place rather than collapsed, and keyed for point
   * lookups; `010_token_metadata.sql` has the measurements.
   */
  readonly postgres: PostgresOptions;
}

export interface PositionsAsyncOptions<TDeps extends unknown[] = unknown[]> extends Pick<
  ModuleMetadata,
  'imports'
> {
  inject?: (InjectionToken | OptionalFactoryDependency)[];
  useFactory: (...args: TDeps) => PositionsOptions | Promise<PositionsOptions>;
}

/** Holds the resolved options so both database modules can read them too. */
@Module({})
class PositionsOptionsModule {}

/**
 * The read side of the position fold, and the stores over the two databases it
 * spans.
 *
 * Deliberately thin next to `SpokeEventsModule`, and the asymmetry is the point.
 * That module owns a processor, a log reader and a write path because ingestion
 * is code; this one is mostly queries, because the fold is materialized views
 * and the database maintains them. The one exception is token metadata, which is
 * fetched rather than folded and therefore has a writer.
 *
 * ```ts
 * const positions = PositionsModule.forRootAsync({
 *   imports: [ConfigModule],
 *   inject: [ConfigService],
 *   useFactory: (config) => ({ clickhouse: … }),
 * });
 * ```
 *
 * **No cursor signing is wired here, and none is exported.** The store deals in
 * `PositionKey`; turning one into an opaque token is the job of whatever
 * publishes the wire format, and the secret that does it is that service's
 * configuration rather than this module's.
 *
 * Re-exports **both** database modules, so an importer can register their
 * health indicators — and, more to the point, read the indexer's cursor —
 * without constructing a second client of either. The API needs Postgres for
 * `SyncStatusStore` anyway; taking it from here is what keeps that one pool.
 */
@Module({})
export class PositionsModule {
  static forRootAsync<TDeps extends unknown[]>(
    options: PositionsAsyncOptions<TDeps>,
  ): DynamicModule {
    const settings: DynamicModule = {
      module: PositionsOptionsModule,
      imports: options.imports ?? [],
      providers: [
        { provide: POSITIONS_OPTIONS, useFactory: options.useFactory, inject: options.inject },
      ],
      exports: [POSITIONS_OPTIONS],
    };

    const clickhouse = ClickHouseModule.forRootAsync({
      imports: [settings],
      inject: [POSITIONS_OPTIONS],
      useFactory: (resolved: PositionsOptions) => resolved.clickhouse,
    });

    const postgres = PostgresModule.forRootAsync({
      imports: [settings],
      inject: [POSITIONS_OPTIONS],
      useFactory: (resolved: PositionsOptions) => resolved.postgres,
    });

    return {
      module: PositionsModule,
      imports: [settings, clickhouse, postgres],
      providers: [
        { provide: POSITION_STORE, useClass: ClickHousePositionStore },
        // The other half of a balance. Same client, same module: nothing values
        // a position without both, so handing out one without the other only
        // makes the incomplete wiring possible.
        { provide: HUB_ASSET_STORE, useClass: ClickHouseHubAssetStore },
        { provide: TOKEN_METADATA_STORE, useClass: PostgresTokenMetadataStore },
        { provide: TOKEN_LISTINGS, useClass: ClickHouseTokenListings },
      ],
      exports: [
        POSITION_STORE,
        HUB_ASSET_STORE,
        TOKEN_METADATA_STORE,
        TOKEN_LISTINGS,
        clickhouse,
        postgres,
      ],
    };
  }
}
