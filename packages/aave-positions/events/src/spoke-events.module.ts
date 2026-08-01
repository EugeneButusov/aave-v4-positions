import {
  Module,
  type DynamicModule,
  type InjectionToken,
  type ModuleMetadata,
  type OptionalFactoryDependency,
} from '@nestjs/common';
import { ClickHouseModule, type ClickHouseOptions } from '@packages/clickhouse';
import {
  CHAIN_CLIENT_OPTIONS,
  LOG_READER,
  ViemLogReader,
  type Address,
  type ChainClientOptions,
  type LogReader,
} from '@packages/indexing';

import { AaveEventProcessor } from './aave-event-processor';
import { ClickHouseEventStore } from './store/clickhouse-event-store';
import { EVENT_STORE, type EventStore } from './store/event-store';

/**
 * The {@link BlockProcessor} this module exports. Hand it to `IndexingModule`'s
 * `processors`; a second Spoke passes its own token, see {@link SpokeEventsAsyncOptions.token}.
 */
export const SPOKE_EVENT_PROCESSOR = Symbol('SPOKE_EVENT_PROCESSOR');

const SPOKE_EVENTS_OPTIONS = Symbol('SPOKE_EVENTS_OPTIONS');

export interface SpokeEventsOptions {
  readonly chainId: number;
  /** Which Spoke to follow. A second Spoke is a second registration. */
  readonly spoke: Address;
  readonly rpc: ChainClientOptions;
  readonly clickhouse: ClickHouseOptions;
}

export interface SpokeEventsAsyncOptions<TDeps extends unknown[] = unknown[]> extends Pick<
  ModuleMetadata,
  'imports'
> {
  inject?: (InjectionToken | OptionalFactoryDependency)[];
  useFactory: (...args: TDeps) => SpokeEventsOptions | Promise<SpokeEventsOptions>;
  /**
   * The token the processor is exported under. Defaults to
   * {@link SPOKE_EVENT_PROCESSOR}; a second Spoke needs its own, because two
   * modules exporting one token into the same importer collide.
   */
  token?: InjectionToken;
}

/** Holds the resolved options so the ClickHouse module can read them too. */
@Module({})
class SpokeEventsOptionsModule {}

/**
 * Everything needed to turn one Spoke's logs into rows: the ClickHouse client,
 * the event store, and the block processor that drives them.
 *
 * The application imports this and names the exported processor, rather than
 * assembling a store, a client and a processor factory itself — which is what
 * keeps the wiring for a second Spoke, or a second chain, to one more call.
 *
 * ```ts
 * const spokeEvents = SpokeEventsModule.forRootAsync({
 *   imports: [ConfigModule],
 *   inject: [ConfigService],
 *   useFactory: (config) => ({ chainId: …, spoke: …, rpc: …, clickhouse: … }),
 * });
 *
 * const indexing = IndexingModule.forRootAsync({
 *   imports: [spokeEvents],
 *   processors: [SPOKE_EVENT_PROCESSOR],
 *   …
 * });
 * ```
 *
 * **It binds its own `LOG_READER`.** A module in `IndexingModule`'s `imports`
 * cannot reach the one that module provides — dynamic-module exports flow
 * outward to importers, not inward — so owning the seam is what lets the
 * processor live here instead of in the application's provider list.
 *
 * Re-exports the ClickHouse module, so an importer can register
 * `ClickHouseHealthIndicator` without constructing a second client.
 */
@Module({})
export class SpokeEventsModule {
  static forRootAsync<TDeps extends unknown[]>(
    options: SpokeEventsAsyncOptions<TDeps>,
  ): DynamicModule {
    const settings: DynamicModule = {
      module: SpokeEventsOptionsModule,
      imports: options.imports ?? [],
      providers: [
        { provide: SPOKE_EVENTS_OPTIONS, useFactory: options.useFactory, inject: options.inject },
      ],
      exports: [SPOKE_EVENTS_OPTIONS],
    };

    const clickhouse = ClickHouseModule.forRootAsync({
      imports: [settings],
      inject: [SPOKE_EVENTS_OPTIONS],
      useFactory: (resolved: SpokeEventsOptions) => resolved.clickhouse,
    });

    const token = options.token ?? SPOKE_EVENT_PROCESSOR;

    return {
      module: SpokeEventsModule,
      imports: [settings, clickhouse],
      providers: [
        {
          provide: CHAIN_CLIENT_OPTIONS,
          useFactory: (resolved: SpokeEventsOptions) => resolved.rpc,
          inject: [SPOKE_EVENTS_OPTIONS],
        },
        { provide: LOG_READER, useClass: ViemLogReader },
        { provide: EVENT_STORE, useClass: ClickHouseEventStore },
        {
          provide: token,
          useFactory: (resolved: SpokeEventsOptions, logs: LogReader, store: EventStore) =>
            new AaveEventProcessor(
              { chainId: resolved.chainId, spoke: resolved.spoke },
              logs,
              store,
            ),
          inject: [SPOKE_EVENTS_OPTIONS, LOG_READER, EVENT_STORE],
        },
      ],
      exports: [token, EVENT_STORE, clickhouse],
    };
  }
}
