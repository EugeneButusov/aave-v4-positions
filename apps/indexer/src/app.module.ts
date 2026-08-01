import { Module } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import { ClickHouseHealthIndicator } from '@packages/clickhouse';
import { IndexerHealthIndicator } from '@packages/indexing';
import { HealthModule, LoggingModule } from '@packages/ops';

import { validateEnv, type Env } from './config/env';
import { indexingSetup } from './indexing.setup';

/**
 * Built once and referenced repeatedly below. Nest identifies a dynamic module
 * by object reference, so a second `indexingSetup()` call would produce a
 * second pair of modules — and with it a second indexing loop racing the same
 * cursor.
 */
const { spokeEvents, indexing } = indexingSetup();

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
