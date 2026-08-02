import { Module } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import { ClickHouseHealthIndicator } from '@packages/clickhouse';
import { IndexerHealthIndicator } from '@packages/indexing';
import { HealthModule, LoggingModule } from '@packages/ops';
import { PostgresHealthIndicator } from '@packages/postgres';

import { validateEnv, type Env } from './config/env';
import { indexingSetup } from './indexing.setup';

/**
 * Built once and referenced twice below. Nest identifies a dynamic module by
 * object reference, so a second `indexingSetup()` call would produce a second
 * pipeline — and with it a second indexing loop racing the same cursor.
 */
const indexing = indexingSetup();

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
      // The same module object referenced above, so this resolves the indicators
      // already constructed rather than building a second graph. Both database
      // probes reach it through that one import: SpokeEventsModule re-exports
      // ClickHouse, the indexing module re-exports both of them in turn.
      imports: [indexing],
      indicators: [IndexerHealthIndicator, ClickHouseHealthIndicator, PostgresHealthIndicator],
    }),
  ],
})
export class AppModule {}
