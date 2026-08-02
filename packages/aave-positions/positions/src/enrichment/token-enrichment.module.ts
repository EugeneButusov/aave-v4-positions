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
  ERC20_METADATA_READER,
  ViemChainClient,
  ViemErc20MetadataReader,
  type ChainClientOptions,
} from '@packages/indexing';
import { PostgresModule, type PostgresOptions } from '@packages/postgres';

import { PostgresTokenMetadataStore } from '../store/postgres-token-metadata-store';
import { ClickHouseTokenListings, TOKEN_LISTINGS } from '../store/token-listing-source';
import { TOKEN_METADATA_STORE } from '../store/token-metadata-store';
import {
  TOKEN_ENRICHMENT_OPTIONS,
  TokenEnrichmentProcessor,
  type TokenEnrichmentOptions,
} from './token-enrichment.processor';

export const TOKEN_ENRICHMENT_PROCESSOR = Symbol('TOKEN_ENRICHMENT_PROCESSOR');

const ENRICHMENT_OPTIONS = Symbol('ENRICHMENT_OPTIONS');

export interface EnrichmentOptions extends TokenEnrichmentOptions {
  readonly rpc: ChainClientOptions;
  readonly clickhouse: ClickHouseOptions;
  readonly postgres: PostgresOptions;
}

export interface EnrichmentAsyncOptions<TDeps extends unknown[] = unknown[]> extends Pick<
  ModuleMetadata,
  'imports'
> {
  inject?: (InjectionToken | OptionalFactoryDependency)[];
  useFactory: (...args: TDeps) => EnrichmentOptions | Promise<EnrichmentOptions>;
  /** Override so a second chain's enrichment does not collide on the token. */
  token?: InjectionToken;
}

/** Holds the resolved options so the database modules can read them too. */
@Module({})
class EnrichmentOptionsModule {}

/**
 * Token metadata enrichment, as a processor the indexing loop drives.
 *
 * Shaped like `SpokeEventsModule` rather than like `PositionsModule`, because
 * it is the same kind of thing: a module that owns a processor and therefore
 * owns the seams that processor needs.
 *
 * **It binds its own `CHAIN_CLIENT` and `ERC20_METADATA_READER**, for the
 * reason `SpokeEventsModule` gives about `LOG_READER`: a module in
 * `IndexingModule`'s `imports` cannot reach what that module provides, because
 * dynamic-module exports flow outward to importers rather than inward. Owning
 * the seam is what lets the processor live here instead of in the
 * application's provider list.
 *
 * ```ts
 * const enrichment = TokenEnrichmentModule.forRootAsync({
 *   imports: [ConfigModule],
 *   inject: [ConfigService],
 *   useFactory: (config) => ({ chainId, sweepIntervalMs, concurrency, rpc, … }),
 * });
 *
 * const indexing = IndexingModule.forRootAsync({
 *   imports: [spokeEvents, hubEvents, enrichment],
 *   processors: [SPOKE_EVENT_PROCESSOR, HUB_EVENT_PROCESSOR, TOKEN_ENRICHMENT_PROCESSOR],
 *   …
 * });
 * ```
 *
 * **Registered after the event processors, and the order is a real dependency
 * for one of its two mechanisms.** The fast path reads what the Hub processor
 * wrote earlier in the same dispatch. The sweep does not, which is what keeps
 * the dependency from being load-bearing — see the processor.
 */
@Module({})
export class TokenEnrichmentModule {
  static forRootAsync<TDeps extends unknown[]>(
    options: EnrichmentAsyncOptions<TDeps>,
  ): DynamicModule {
    const settings: DynamicModule = {
      module: EnrichmentOptionsModule,
      imports: options.imports ?? [],
      providers: [
        { provide: ENRICHMENT_OPTIONS, useFactory: options.useFactory, inject: options.inject },
      ],
      exports: [ENRICHMENT_OPTIONS],
    };

    const clickhouse = ClickHouseModule.forRootAsync({
      imports: [settings],
      inject: [ENRICHMENT_OPTIONS],
      useFactory: (resolved: EnrichmentOptions) => resolved.clickhouse,
    });

    const postgres = PostgresModule.forRootAsync({
      imports: [settings],
      inject: [ENRICHMENT_OPTIONS],
      useFactory: (resolved: EnrichmentOptions) => resolved.postgres,
    });

    const token = options.token ?? TOKEN_ENRICHMENT_PROCESSOR;

    return {
      module: TokenEnrichmentModule,
      imports: [settings, clickhouse, postgres],
      providers: [
        {
          provide: CHAIN_CLIENT_OPTIONS,
          useFactory: (resolved: EnrichmentOptions) => resolved.rpc,
          inject: [ENRICHMENT_OPTIONS],
        },
        { provide: CHAIN_CLIENT, useClass: ViemChainClient },
        { provide: ERC20_METADATA_READER, useClass: ViemErc20MetadataReader },
        { provide: TOKEN_LISTINGS, useClass: ClickHouseTokenListings },
        { provide: TOKEN_METADATA_STORE, useClass: PostgresTokenMetadataStore },
        {
          provide: TOKEN_ENRICHMENT_OPTIONS,
          useFactory: (resolved: EnrichmentOptions): TokenEnrichmentOptions => ({
            chainId: resolved.chainId,
            sweepIntervalMs: resolved.sweepIntervalMs,
            concurrency: resolved.concurrency,
          }),
          inject: [ENRICHMENT_OPTIONS],
        },
        { provide: token, useClass: TokenEnrichmentProcessor },
      ],
      exports: [token, TOKEN_METADATA_STORE, postgres],
    };
  }
}
