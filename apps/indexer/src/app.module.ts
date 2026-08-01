import { Module } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import { HealthModule, LoggingModule } from '@aave-v4-positions/ops';

import { BLOCK_HEADER_STORE } from './indexing/reorg/block-header-store';
import { validateEnv, type Env } from './config/env';
import { HashChainReorgDetector } from './indexing/reorg/hash-chain-reorg-detector';
import { IndexerHealthIndicator } from './indexing/observability/indexer.health-indicator';
import { IndexingModule } from './indexing/indexing.module';
import { InMemoryBlockHeaderStore } from './indexing/reorg/in-memory-block-header-store';
import { InMemoryCursorStore } from './indexing/cursor/in-memory-cursor-store';
import { LoggingBlockProcessor } from './indexing/processors/logging-block-processor';

/**
 * Built once and referenced twice below. Nest identifies a dynamic module by
 * object reference, so a second `forRootAsync()` call would produce a second
 * module — and with it a second indexing loop racing the same cursor.
 */
const indexing = IndexingModule.forRootAsync({
  imports: [ConfigModule],
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
  // The processor is still a placeholder; the real event-decoding one replaces
  // it without the loop changing.
  processors: [LoggingBlockProcessor],
  reorgDetector: HashChainReorgDetector,
  cursorStore: InMemoryCursorStore,
  providers: [
    LoggingBlockProcessor,
    HashChainReorgDetector,
    // Both stores are in memory, which is the pairing that keeps the detector
    // honest: neither the cursor nor the retention window survives a restart,
    // so there is no resume to get wrong. A durable cursor store arriving
    // without a durable window would be worse than either alone — see
    // BlockHeaderStore.
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
      imports: [indexing],
      indicators: [IndexerHealthIndicator],
    }),
  ],
})
export class AppModule {}
