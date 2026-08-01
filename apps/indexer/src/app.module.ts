import { Module } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import { SPOKE_EVENT_PROCESSOR, SpokeEventsModule } from '@aave-positions/events';
import { ClickHouseHealthIndicator } from '@packages/clickhouse';
import {
  BLOCK_HEADER_STORE,
  HashChainReorgDetector,
  IndexerHealthIndicator,
  IndexingModule,
  InMemoryBlockHeaderStore,
  InMemoryCursorStore,
} from '@packages/indexing';
import { HealthModule, LoggingModule } from '@packages/ops';

import { validateEnv, type Env } from './config/env';

/**
 * Everything Aave-specific: the client, the store and the processor that fills
 * it. The application names which Spoke to follow and nothing else.
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

/**
 * Built once and referenced twice below. Nest identifies a dynamic module by
 * object reference, so a second `forRootAsync()` call would produce a second
 * module — and with it a second indexing loop racing the same cursor.
 */
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
    autoStart: config.get('INDEXER_AUTOSTART', { infer: true }),
  }),
  processors: [SPOKE_EVENT_PROCESSOR],
  reorgDetector: HashChainReorgDetector,
  cursorStore: InMemoryCursorStore,
  providers: [
    HashChainReorgDetector,
    // Both in memory, and they want to become durable together: a cursor that
    // outlives the process while the window does not would name a resume point
    // nothing can vet. See BlockHeaderStore.
    { provide: BLOCK_HEADER_STORE, useClass: InMemoryBlockHeaderStore },
    InMemoryCursorStore,
  ],
});

@Module({
  imports: [
    ConfigModule.forRoot({
      isGlobal: true,
      cache: true,
      // Deployed environments inject variables directly; the dotenv file is a
      // local-development convenience only, and tests stay hermetic.
      envFilePath: ['.env'],
      ignoreEnvFile: process.env['NODE_ENV'] === 'test',
      validate: validateEnv,
    }),
    LoggingModule.forRootAsync({
      imports: [ConfigModule],
      inject: [ConfigService],
      useFactory: (config: ConfigService<Env, true>) => ({
        service: 'indexer',
        level: config.get('LOG_LEVEL', { infer: true }),
        pretty: config.get('LOG_PRETTY', { infer: true }),
        // One log stream can carry several chains; tag every line with which.
        base: { chainId: config.get('CHAIN_ID', { infer: true }) },
      }),
    }),
    indexing,
    HealthModule.forRoot({
      // The same module objects referenced above, so this resolves the
      // indicators already constructed rather than building a second graph.
      // SpokeEventsModule re-exports ClickHouse, so its probe comes from there.
      imports: [indexing, spokeEvents],
      indicators: [IndexerHealthIndicator, ClickHouseHealthIndicator],
    }),
  ],
})
export class AppModule {}
