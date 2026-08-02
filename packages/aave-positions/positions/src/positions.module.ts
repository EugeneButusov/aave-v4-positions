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
import { PositionCursorCodec } from './store/position-cursor';
import { POSITION_STORE } from './store/position-store';

const POSITIONS_OPTIONS = Symbol('POSITIONS_OPTIONS');

export interface PositionsOptions {
  readonly clickhouse: ClickHouseOptions;
  /**
   * Signs pagination cursors, so one cannot be altered or carried to a
   * different listing. **Must be identical across replicas** — a per-process
   * secret means a cursor issued by one pod is rejected by the next, which
   * shows up as pagination that fails only under load.
   */
  readonly cursorSecret: string;
}

export interface PositionsAsyncOptions<TDeps extends unknown[] = unknown[]> extends Pick<
  ModuleMetadata,
  'imports'
> {
  inject?: (InjectionToken | OptionalFactoryDependency)[];
  useFactory: (...args: TDeps) => PositionsOptions | Promise<PositionsOptions>;
}

/** Holds the resolved options so the ClickHouse module can read them too. */
@Module({})
class PositionsOptionsModule {}

/**
 * The read side of the position fold: a ClickHouse client and the store over it.
 *
 * Deliberately thin next to `SpokeEventsModule`, and the asymmetry is the point.
 * That module owns a processor, a log reader and a write path because ingestion
 * is code; this one owns a query, because the fold is materialized views and the
 * database maintains it. There is nothing here to start, and nothing to keep in
 * step with the indexer.
 *
 * ```ts
 * const positions = PositionsModule.forRootAsync({
 *   imports: [ConfigModule],
 *   inject: [ConfigService],
 *   useFactory: (config) => ({ clickhouse: … }),
 * });
 * ```
 *
 * Re-exports the ClickHouse module, so an importer can register
 * `ClickHouseHealthIndicator` without constructing a second client.
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
        {
          provide: PositionCursorCodec,
          useFactory: (resolved: PositionsOptions) =>
            new PositionCursorCodec(resolved.cursorSecret),
          inject: [POSITIONS_OPTIONS],
        },
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
