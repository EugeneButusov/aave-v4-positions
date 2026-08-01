import type { DynamicModule } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import { SPOKE_EVENT_PROCESSOR, SpokeEventsModule } from '@aave-positions/events';
import {
  BLOCK_HEADER_STORE,
  HashChainReorgDetector,
  IndexingModule,
  InMemoryBlockHeaderStore,
  InMemoryCursorStore,
} from '@packages/indexing';

import type { Env } from './config/env';

export interface IndexingSetup {
  /**
   * Held separately because the health probes reference it too, and Nest
   * identifies a dynamic module by object reference — resolving the ClickHouse
   * indicator from a second `forRootAsync()` call would build a second graph.
   */
  readonly spokeEvents: DynamicModule;
  readonly indexing: DynamicModule;
}

/**
 * The indexing pipeline, in one place because two entry points need it
 * identical: the daemon in `AppModule`, and the backfill command.
 *
 * **The processor list is why this is shared rather than repeated.** A
 * processor added to one and forgotten in the other would not fail — a backfill
 * would quietly run fewer processors over the range and report success, which
 * is the worst shape this class of bug can take.
 *
 * **Call this once per process and destructure what it returns.** A second call
 * is a second pair of modules, and with it a second `IndexerService`. The two
 * roots each call it once, and never load together.
 */
export function indexingSetup(overrides: { readonly autoStart?: boolean } = {}): IndexingSetup {
  /**
   * Everything Aave-specific: the client, the store and the processor that
   * fills it. The application names which Spoke to follow and nothing else.
   */
  const spokeEvents = SpokeEventsModule.forRootAsync({
    imports: [ConfigModule],
    inject: [ConfigService],
    useFactory: (config: ConfigService<Env, true>) => ({
      chainId: config.get('CHAIN_ID', { infer: true }),
      spoke: config.get('MAIN_SPOKE_ADDRESS', { infer: true }),
      rpc: {
        rpcUrls: config.get('RPC_URLS', { infer: true }),
        rpcTimeoutMs: config.get('INDEXER_RPC_TIMEOUT_MS', { infer: true }),
      },
      clickhouse: {
        url: config.get('CLICKHOUSE_URL', { infer: true }),
        database: config.get('CLICKHOUSE_DATABASE', { infer: true }),
        username: config.get('CLICKHOUSE_USER', { infer: true }),
        password: config.get('CLICKHOUSE_PASSWORD', { infer: true }),
      },
    }),
  });

  const indexing = IndexingModule.forRootAsync({
    imports: [ConfigModule, spokeEvents],
    inject: [ConfigService],
    useFactory: (config: ConfigService<Env, true>) => ({
      chainId: config.get('CHAIN_ID', { infer: true }),
      rpcUrls: config.get('RPC_URLS', { infer: true }),
      rpcTimeoutMs: config.get('INDEXER_RPC_TIMEOUT_MS', { infer: true }),
      finalityDepth: config.get('FINALITY_DEPTH', { infer: true }),
      startBlock: config.get('INDEXER_START_BLOCK', { infer: true }),
      maxRangeSize: config.get('INDEXER_MAX_RANGE_SIZE', { infer: true }),
      pollIntervalMs: config.get('INDEXER_POLL_INTERVAL_MS', { infer: true }),
      stallThresholdMs: config.get('INDEXER_STALL_THRESHOLD_MS', { infer: true }),
      // A command overrides this to `false`. Left to the environment otherwise,
      // where it is how a pod boots its probes without indexing.
      autoStart: overrides.autoStart ?? config.get('INDEXER_AUTOSTART', { infer: true }),
    }),
    processors: [SPOKE_EVENT_PROCESSOR],
    reorgDetector: HashChainReorgDetector,
    cursorStore: InMemoryCursorStore,
    providers: [
      HashChainReorgDetector,
      // Both in memory, and they want to become durable together: a cursor that
      // outlives the process while the window does not would name a resume
      // point nothing can vet. See BlockHeaderStore.
      { provide: BLOCK_HEADER_STORE, useClass: InMemoryBlockHeaderStore },
      InMemoryCursorStore,
    ],
  });

  return { spokeEvents, indexing };
}
