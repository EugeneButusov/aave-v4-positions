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

import { AaveEventProcessor, hubEventSource } from './aave-event-processor';
import { ClickHouseHubEventStore } from './store/clickhouse-hub-event-store';
import { HUB_EVENT_STORE, type EventStore } from './store/event-store';

/**
 * The {@link BlockProcessor} this module exports. Hand it to `IndexingModule`'s
 * `processors`; a second Hub passes its own token, see {@link HubEventsAsyncOptions.token}.
 */
export const HUB_EVENT_PROCESSOR = Symbol('HUB_EVENT_PROCESSOR');

const HUB_EVENTS_OPTIONS = Symbol('HUB_EVENTS_OPTIONS');

export interface HubEventsOptions {
  readonly chainId: number;
  /** Which Hub to follow. A second Hub is a second registration. */
  readonly hub: Address;
  readonly rpc: ChainClientOptions;
  readonly clickhouse: ClickHouseOptions;
}

export interface HubEventsAsyncOptions<TDeps extends unknown[] = unknown[]> extends Pick<
  ModuleMetadata,
  'imports'
> {
  inject?: (InjectionToken | OptionalFactoryDependency)[];
  useFactory: (...args: TDeps) => HubEventsOptions | Promise<HubEventsOptions>;
  /**
   * The token the processor is exported under. Defaults to
   * {@link HUB_EVENT_PROCESSOR}; a second Hub needs its own, because two
   * modules exporting one token into the same importer collide.
   */
  token?: InjectionToken;
}

/** Holds the resolved options so the ClickHouse module can read them too. */
@Module({})
class HubEventsOptionsModule {}

/**
 * Everything needed to turn one Hub's logs into rows: the ClickHouse client,
 * the event store, and the block processor that drives them.
 *
 * The Hub twin of `SpokeEventsModule`, and a separate module rather than an
 * option on that one. The two contracts are independent streams with
 * independent ABIs and independent tables, and keeping them separate is what
 * makes "a second Spoke" and "a second Hub" the same one-line change. The cost
 * is a second ClickHouse client and a second log reader, which is the trade
 * `SpokeEventsModule` already makes and documents — a module inside
 * `IndexingModule`'s `imports` cannot reach the providers that module supplies.
 *
 * ```ts
 * const hubEvents = HubEventsModule.forRootAsync({
 *   imports: [ConfigModule],
 *   inject: [ConfigService],
 *   useFactory: (config) => ({ chainId: …, hub: …, rpc: …, clickhouse: … }),
 * });
 *
 * const indexing = IndexingModule.forRootAsync({
 *   imports: [spokeEvents, hubEvents],
 *   processors: [SPOKE_EVENT_PROCESSOR, HUB_EVENT_PROCESSOR],
 *   …
 * });
 * ```
 */
@Module({})
export class HubEventsModule {
  static forRootAsync<TDeps extends unknown[]>(
    options: HubEventsAsyncOptions<TDeps>,
  ): DynamicModule {
    const settings: DynamicModule = {
      module: HubEventsOptionsModule,
      imports: options.imports ?? [],
      providers: [
        { provide: HUB_EVENTS_OPTIONS, useFactory: options.useFactory, inject: options.inject },
      ],
      exports: [HUB_EVENTS_OPTIONS],
    };

    const clickhouse = ClickHouseModule.forRootAsync({
      imports: [settings],
      inject: [HUB_EVENTS_OPTIONS],
      useFactory: (resolved: HubEventsOptions) => resolved.clickhouse,
    });

    const token = options.token ?? HUB_EVENT_PROCESSOR;

    return {
      module: HubEventsModule,
      imports: [settings, clickhouse],
      providers: [
        {
          provide: CHAIN_CLIENT_OPTIONS,
          useFactory: (resolved: HubEventsOptions) => resolved.rpc,
          inject: [HUB_EVENTS_OPTIONS],
        },
        { provide: LOG_READER, useClass: ViemLogReader },
        { provide: HUB_EVENT_STORE, useClass: ClickHouseHubEventStore },
        {
          provide: token,
          useFactory: (resolved: HubEventsOptions, logs: LogReader, store: EventStore) =>
            new AaveEventProcessor(hubEventSource(resolved.chainId, resolved.hub), logs, store),
          inject: [HUB_EVENTS_OPTIONS, LOG_READER, HUB_EVENT_STORE],
        },
      ],
      exports: [token, HUB_EVENT_STORE, clickhouse],
    };
  }
}
