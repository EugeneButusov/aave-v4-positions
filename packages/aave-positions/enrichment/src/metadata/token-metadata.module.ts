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
import { PendingTokens } from './pending-tokens';
import {
  TOKEN_METADATA_OPTIONS,
  TokenMetadataFiller,
  type TokenMetadataOptions,
} from './token-metadata.filler';

const METADATA_OPTIONS = Symbol('METADATA_OPTIONS');

export interface MetadataOptions extends TokenMetadataOptions {
  /**
   * Where ingestion drops newly listed tokens.
   *
   * Passed in rather than provided here, because the other end of it is the
   * Hub event processor in a module this one cannot reach — dynamic-module
   * exports flow outward to importers, not sideways. The composition root owns
   * the instance and hands the same one to both.
   */
  readonly pending: PendingTokens;
  readonly rpc: ChainClientOptions;
  readonly clickhouse: ClickHouseOptions;
  readonly postgres: PostgresOptions;
}

export interface MetadataAsyncOptions<TDeps extends unknown[] = unknown[]> extends Pick<
  ModuleMetadata,
  'imports'
> {
  inject?: (InjectionToken | OptionalFactoryDependency)[];
  useFactory: (...args: TDeps) => MetadataOptions | Promise<MetadataOptions>;
}

/** Holds the resolved options so the database modules can read them too. */
@Module({})
class MetadataOptionsModule {}

/**
 * What each token calls itself, filled in automatically.
 *
 * **A peer of the indexing pipeline rather than a part of it**, the same shape
 * `ReservePriceModule` has and for a related reason. Reading an ERC-20 has no
 * more to do with a block range than reading an oracle does: the loop was never
 * the source of the work, only an incidental clock — and one that stops when it
 * is caught up, stalled, or not started. Discovery still comes from the event
 * that lists a token, through {@link PendingTokens}; what left the loop is the
 * *waking*, which the buffer now does directly.
 *
 * **It binds its own `CHAIN_CLIENT` and `ERC20_METADATA_READER`**, for the
 * reason `SpokeEventsModule` gives about `LOG_READER`: a module cannot reach
 * what a sibling provides, because dynamic-module exports flow outward to
 * importers rather than sideways. Owning the seam is what lets the filler live
 * here instead of in the application's provider list.
 *
 * ```ts
 * const metadata = TokenMetadataModule.forRootAsync({
 *   imports: [ConfigModule],
 *   inject: [ConfigService],
 *   useFactory: (config) => ({ chainId, pending, retryDelayMs, concurrency, autoStart, … }),
 * });
 * ```
 *
 * Importing it is enough to start it — Nest instantiates providers eagerly, and
 * `onApplicationBootstrap` does the rest. `autoStart: false` is how a graph that
 * only wants the ports (the one-shot command, a hermetic test) declines the
 * filler.
 */
@Module({})
export class TokenMetadataModule {
  static forRootAsync<TDeps extends unknown[]>(
    options: MetadataAsyncOptions<TDeps>,
  ): DynamicModule {
    const settings: DynamicModule = {
      module: MetadataOptionsModule,
      imports: options.imports ?? [],
      providers: [
        { provide: METADATA_OPTIONS, useFactory: options.useFactory, inject: options.inject },
      ],
      exports: [METADATA_OPTIONS],
    };

    const clickhouse = ClickHouseModule.forRootAsync({
      imports: [settings],
      inject: [METADATA_OPTIONS],
      useFactory: (resolved: MetadataOptions) => resolved.clickhouse,
    });

    const postgres = PostgresModule.forRootAsync({
      imports: [settings],
      inject: [METADATA_OPTIONS],
      useFactory: (resolved: MetadataOptions) => resolved.postgres,
    });

    return {
      module: TokenMetadataModule,
      imports: [settings, clickhouse, postgres],
      providers: [
        {
          provide: CHAIN_CLIENT_OPTIONS,
          useFactory: (resolved: MetadataOptions) => resolved.rpc,
          inject: [METADATA_OPTIONS],
        },
        { provide: CHAIN_CLIENT, useClass: ViemChainClient },
        { provide: ERC20_METADATA_READER, useClass: ViemErc20MetadataReader },
        { provide: TOKEN_LISTINGS, useClass: ClickHouseTokenListings },
        { provide: TOKEN_METADATA_STORE, useClass: PostgresTokenMetadataStore },
        {
          provide: TOKEN_METADATA_OPTIONS,
          useFactory: (resolved: MetadataOptions): TokenMetadataOptions => ({
            chainId: resolved.chainId,
            retryDelayMs: resolved.retryDelayMs,
            concurrency: resolved.concurrency,
            autoStart: resolved.autoStart,
          }),
          inject: [METADATA_OPTIONS],
        },
        {
          provide: PendingTokens,
          useFactory: (resolved: MetadataOptions) => resolved.pending,
          inject: [METADATA_OPTIONS],
        },
        TokenMetadataFiller,
      ],
      // Every seam the filler uses, not just the store. `enrich:tokens` is a
      // provider in *its* module, and Nest resolves a provider's dependencies
      // from the module it is declared in — so a token missing here is a boot
      // failure in a command no spec covers rather than a compile error.
      exports: [
        TokenMetadataFiller,
        TOKEN_METADATA_STORE,
        TOKEN_LISTINGS,
        ERC20_METADATA_READER,
        CHAIN_CLIENT,
        postgres,
      ],
    };
  }
}
