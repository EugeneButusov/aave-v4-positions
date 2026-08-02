import {
  Module,
  type DynamicModule,
  type InjectionToken,
  type ModuleMetadata,
  type OptionalFactoryDependency,
} from '@nestjs/common';
import { ClickHouseModule, type ClickHouseOptions } from '@packages/clickhouse';

import { ClickHouseHubAssetStore } from './store/clickhouse-hub-asset-store';
import { ClickHousePositionStore } from './store/clickhouse-position-store';
import { HUB_ASSET_STORE } from './store/hub-asset-store';
import { POSITION_STORE } from './store/position-store';

const POSITIONS_OPTIONS = Symbol('POSITIONS_OPTIONS');

export interface PositionsOptions {
  readonly clickhouse: ClickHouseOptions;
}

export interface PositionsAsyncOptions<TDeps extends unknown[] = unknown[]> extends Pick<
  ModuleMetadata,
  'imports'
> {
  inject?: (InjectionToken | OptionalFactoryDependency)[];
  useFactory: (...args: TDeps) => PositionsOptions | Promise<PositionsOptions>;
}

/** Holds the resolved options so the database module can read them too. */
@Module({})
class PositionsOptionsModule {}

/**
 * The read side of the position fold, and the stores over it.
 *
 * Deliberately thin next to `SpokeEventsModule`, and the asymmetry is the
 * point. That module owns a processor, a log reader and a write path because
 * ingestion is code; this one is queries, because the fold is materialized
 * views and the database maintains them. **Nothing in this package writes.**
 *
 * **One database again.** It briefly spanned two, while token metadata lived
 * here — the one table that was fetched rather than folded. It is now
 * `@packages/token-metadata`, which is where that shape belongs: a small keyed
 * dimension, replaced in place, filled by a worker rather than by a view, and
 * about an ERC-20 rather than about Aave. What is left here is the fold, and
 * the fold is ClickHouse.
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
 * Re-exports the ClickHouse module so an importer can register its health
 * indicator without constructing a second client.
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

    return {
      module: PositionsModule,
      imports: [settings, clickhouse],
      providers: [
        { provide: POSITION_STORE, useClass: ClickHousePositionStore },
        // The other half of a balance. Same client, same module: nothing values
        // a position without both, so handing out one without the other only
        // makes the incomplete wiring possible.
        { provide: HUB_ASSET_STORE, useClass: ClickHouseHubAssetStore },
      ],
      exports: [POSITION_STORE, HUB_ASSET_STORE, clickhouse],
    };
  }
}
