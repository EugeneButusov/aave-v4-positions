import {
  Module,
  type DynamicModule,
  type InjectionToken,
  type ModuleMetadata,
  type OptionalFactoryDependency,
} from '@nestjs/common';

import { CHAIN_CLIENT, CHAIN_CLIENT_OPTIONS } from '../chain/chain-client';
import { LOG_READER } from '../chain/log-reader';
import { ViemChainClient } from '../chain/viem-chain-client';
import { ViemLogReader } from '../chain/viem-log-reader';
import { BLOCK_PROCESSORS, type BlockProcessor } from './processors/block-processor';
import { CURSOR_STORE } from './cursor/cursor-store';
import { IndexerHealthIndicator } from './observability/indexer.health-indicator';
import { IndexerService } from './indexer.service';
import { IndexerStatus } from './observability/indexer-status';
import { INDEXING_OPTIONS, type IndexingOptions } from './indexing.options';
import { REORG_DETECTOR } from './reorg/reorg-detector';

/**
 * `TDeps` is inferred from the factory's own parameter list, so the injected
 * dependencies stay typed end to end rather than collapsing to `any[]`.
 */
export interface IndexingAsyncOptions<TDeps extends unknown[] = unknown[]> extends Pick<
  ModuleMetadata,
  'imports' | 'providers'
> {
  /**
   * Injection tokens for the factory, not providers — this binds a factory
   * provider directly, where `LoggingModule` hands its `inject` to a
   * third-party module that accepts a looser shape.
   */
  inject?: (InjectionToken | OptionalFactoryDependency)[];
  useFactory: (...args: TDeps) => IndexingOptions | Promise<IndexingOptions>;

  /**
   * Tokens resolving to {@link BlockProcessor}, dispatched in this order. Each
   * is resolved through DI, so a processor can inject a database connection or
   * the raw viem client. Declare them in `providers`, or import a module that
   * exports them.
   */
  processors?: InjectionToken[];

  reorgDetector: InjectionToken;
  cursorStore: InjectionToken;
}

/**
 * Wires the indexing loop to a chain, a reorg detector, a cursor store and a
 * list of processors.
 *
 * ```ts
 * const indexing = IndexingModule.forRootAsync({
 *   inject: [ConfigService],
 *   useFactory: (config) => ({ chainId: 1, rpcUrls: [...], ... }),
 *   processors: [EventIngestProcessor],
 *   reorgDetector: HashChainReorgDetector,
 *   cursorStore: DatabaseCursorStore,
 *   providers: [EventIngestProcessor, HashChainReorgDetector, DatabaseCursorStore],
 * });
 * ```
 *
 * **Call `forRootAsync` once per application and reuse the returned object.**
 * Nest identifies a dynamic module by reference, so calling it twice — say once
 * for `imports` and once to hand the health indicator to `HealthModule` —
 * produces two modules, two `IndexerService` instances and two loops racing the
 * same cursor. Assign it to a `const` and pass that same value everywhere.
 *
 * Multiple chains would need per-chain tokens rather than class tokens, since
 * the second registration silently overwrites `IndexerService` in the importing
 * module's provider map. Not built: the scope is one chain.
 */
@Module({})
export class IndexingModule {
  static forRootAsync<TDeps extends unknown[]>(
    options: IndexingAsyncOptions<TDeps>,
  ): DynamicModule {
    const { imports = [], providers = [], processors = [] } = options;

    return {
      module: IndexingModule,
      imports,
      providers: [
        ...providers,
        {
          provide: INDEXING_OPTIONS,
          useFactory: options.useFactory,
          inject: options.inject,
        },
        {
          provide: BLOCK_PROCESSORS,
          // Resolved positionally from `inject`, which is what lets each
          // processor be a fully injected service rather than a bare value.
          useFactory: (...resolved: BlockProcessor[]) => resolved,
          inject: processors,
        },
        { provide: REORG_DETECTOR, useExisting: options.reorgDetector },
        { provide: CURSOR_STORE, useExisting: options.cursorStore },
        // IndexingOptions already carries the provider list and timeout, so the
        // chain layer reads the same object under its own narrower token rather
        // than taking a dependency on the indexing options type.
        { provide: CHAIN_CLIENT_OPTIONS, useExisting: INDEXING_OPTIONS },
        // One adapter per port, so what a consumer can do is decided by the
        // token it injects: the loop cannot read a log, and a processor cannot
        // move the loop. Constructing either does no I/O, so this cannot make
        // boot depend on a reachable node — the chain id is checked on the
        // first iteration.
        //
        // They hold separate transports. Both are built from the same ordered
        // provider list and try it in the same order, so they only diverge if
        // one fails over and the other does not; the loop clamps the head
        // monotonically, which is what keeps that from reading as a reorg.
        { provide: CHAIN_CLIENT, useClass: ViemChainClient },
        { provide: LOG_READER, useClass: ViemLogReader },
        IndexerStatus,
        IndexerService,
        IndexerHealthIndicator,
      ],
      exports: [
        IndexerService,
        IndexerStatus,
        IndexerHealthIndicator,
        INDEXING_OPTIONS,
        CHAIN_CLIENT,
        LOG_READER,
      ],
    };
  }
}
