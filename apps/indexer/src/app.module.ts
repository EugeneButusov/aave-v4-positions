import { Module } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import { HealthModule, LoggingModule } from '@aave-v4-positions/ops';

import { validateEnv, type Env } from './config/env';
import { IndexerHealthIndicator } from './indexing/observability/indexer.health-indicator';
import { IndexingModule } from './indexing/indexing.module';
import { InMemoryCursorStore } from './indexing/cursor/in-memory-cursor-store';
import { LoggingBlockProcessor } from './indexing/processors/logging-block-processor';
import { NoopReorgDetector } from './indexing/reorg/noop-reorg-detector';

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
  // Placeholders. The real event-decoding processor and the hash-chain detector
  // replace these without the loop changing.
  processors: [LoggingBlockProcessor],
  reorgDetector: NoopReorgDetector,
  cursorStore: InMemoryCursorStore,
  providers: [LoggingBlockProcessor, NoopReorgDetector, InMemoryCursorStore],
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
