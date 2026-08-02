import {
  Module,
  type DynamicModule,
  type InjectionToken,
  type ModuleMetadata,
  type OptionalFactoryDependency,
} from '@nestjs/common';
import { ClickHouseModule, type ClickHouseOptions } from '@packages/clickhouse';
import {
  CHAIN_CLIENT,
  CHAIN_CLIENT_OPTIONS,
  ViemChainClient,
  type ChainClientOptions,
} from '@packages/indexing';
import { PostgresModule, type PostgresOptions } from '@packages/postgres';

import { PostgresReservePriceStore } from './store/postgres-reserve-price-store';
import { ClickHouseReserveListings, RESERVE_LISTINGS } from './store/reserve-listing-source';
import { RESERVE_PRICE_STORE } from './store/reserve-price-store';
import { RESERVE_PRICE_READER } from './oracle/reserve-price-reader';
import {
  RESERVE_PRICE_OPTIONS,
  ReservePriceRefresher,
  type ReservePriceOptions,
} from './reserve-price.refresher';
import { ViemReservePriceReader } from './oracle/viem-reserve-price-reader';

const PRICING_OPTIONS = Symbol('PRICING_OPTIONS');

export interface PricingOptions extends ReservePriceOptions {
  readonly rpc: ChainClientOptions;
  readonly clickhouse: ClickHouseOptions;
  readonly postgres: PostgresOptions;
}

export interface PricingAsyncOptions<TDeps extends unknown[] = unknown[]> extends Pick<
  ModuleMetadata,
  'imports'
> {
  inject?: (InjectionToken | OptionalFactoryDependency)[];
  useFactory: (...args: TDeps) => PricingOptions | Promise<PricingOptions>;
}

/** Holds the resolved options so the database modules can read them too. */
@Module({})
class PricingOptionsModule {}

/**
 * Reserve prices, refreshed on a timer this module owns.
 *
 * **A peer of the indexing pipeline rather than a part of it**, which is the
 * one structural difference from `TokenEnrichmentModule`. That one contributes
 * a `BlockProcessor`, because a token's metadata changes on chain and the event
 * that changes it is a sound trigger. An oracle's feeds move off chain on their
 * own schedule, so the trigger here is wall-clock time and nothing about it
 * belongs to the loop — see {@link ReservePriceRefresher} for what went wrong
 * when it was a processor.
 *
 * It still **binds its own `CHAIN_CLIENT` and `RESERVE_PRICE_READER`**, for the
 * reason the event modules give: a module cannot reach what a sibling provides,
 * and owning the seam is what lets the refresher live here rather than in the
 * application's provider list.
 *
 * ```ts
 * const pricing = ReservePriceModule.forRootAsync({
 *   imports: [ConfigModule],
 *   inject: [ConfigService],
 *   useFactory: (config) => ({ chainId, spoke, oracle, refreshMs, retryMs, autoStart, rpc, … }),
 * });
 * ```
 *
 * Importing it is enough to start the timer — Nest instantiates providers
 * eagerly, and `onApplicationBootstrap` does the rest. `autoStart: false` is
 * how a graph that only wants the ports (the one-shot command, a hermetic test)
 * declines the poller.
 *
 * **One Spoke per registration**, and the oracle comes with it. `IAaveOracleV4`
 * belongs to a single Spoke and is keyed by `reserveId`, so a second Spoke is a
 * second oracle over a disjoint id space (§12.3) — pairing them here is what
 * stops a reserve id being priced by the wrong contract.
 *
 * **It builds its own Postgres pool**, after the cursor's and enrichment's,
 * which is the cost of every module owning its own seam. Three pools at the
 * driver default is twelve connections from one process; on a managed instance
 * with a hard cap that is worth consolidating, and it is tracked separately
 * rather than solved by making this module the odd one out.
 */
@Module({})
export class ReservePriceModule {
  static forRootAsync<TDeps extends unknown[]>(options: PricingAsyncOptions<TDeps>): DynamicModule {
    const settings: DynamicModule = {
      module: PricingOptionsModule,
      imports: options.imports ?? [],
      providers: [
        { provide: PRICING_OPTIONS, useFactory: options.useFactory, inject: options.inject },
      ],
      exports: [PRICING_OPTIONS],
    };

    const clickhouse = ClickHouseModule.forRootAsync({
      imports: [settings],
      inject: [PRICING_OPTIONS],
      useFactory: (resolved: PricingOptions) => resolved.clickhouse,
    });

    const postgres = PostgresModule.forRootAsync({
      imports: [settings],
      inject: [PRICING_OPTIONS],
      useFactory: (resolved: PricingOptions) => resolved.postgres,
    });

    return {
      module: ReservePriceModule,
      imports: [settings, clickhouse, postgres],
      providers: [
        {
          provide: CHAIN_CLIENT_OPTIONS,
          useFactory: (resolved: PricingOptions) => resolved.rpc,
          inject: [PRICING_OPTIONS],
        },
        { provide: CHAIN_CLIENT, useClass: ViemChainClient },
        { provide: RESERVE_PRICE_READER, useClass: ViemReservePriceReader },
        { provide: RESERVE_LISTINGS, useClass: ClickHouseReserveListings },
        { provide: RESERVE_PRICE_STORE, useClass: PostgresReservePriceStore },
        {
          provide: RESERVE_PRICE_OPTIONS,
          useFactory: (resolved: PricingOptions): ReservePriceOptions => ({
            chainId: resolved.chainId,
            spoke: resolved.spoke,
            oracle: resolved.oracle,
            refreshMs: resolved.refreshMs,
            retryMs: resolved.retryMs,
            autoStart: resolved.autoStart,
          }),
          inject: [PRICING_OPTIONS],
        },
        ReservePriceRefresher,
      ],
      // Every seam the refresher uses, not just the store. A one-shot command
      // built on the same ports is a provider in *its* module, and Nest resolves
      // a provider's dependencies from the module it is declared in — so a token
      // missing here is a boot failure in a command no spec covers rather than a
      // compile error. The indexer's `price.module.spec.ts` proves the graph.
      exports: [
        ReservePriceRefresher,
        RESERVE_PRICE_STORE,
        RESERVE_LISTINGS,
        RESERVE_PRICE_READER,
        CHAIN_CLIENT,
        postgres,
      ],
    };
  }
}
