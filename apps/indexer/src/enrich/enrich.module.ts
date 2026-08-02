import { Module } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import { TokenEnrichmentModule } from '@aave-positions/positions';
import { LoggingModule } from '@packages/ops';

import { validateEnv, type Env } from '../config/env';
import { TokenEnricher } from './token-enricher';

/**
 * The one-shot enrichment command's graph.
 *
 * Deliberately **not** `indexingSetup()`. That builds the cursor store, the
 * reorg detector and an `IndexerService` that would have to be remembered to
 * disable; this job needs a chain client and two databases, which is what
 * `TokenEnrichmentModule` already assembles for the processor.
 *
 * The sweep interval is zero here: a command run by hand is always due.
 */
@Module({
  imports: [
    ConfigModule.forRoot({
      isGlobal: true,
      cache: true,
      envFilePath: ['.env'],
      ignoreEnvFile: process.env['NODE_ENV'] === 'test',
      validate: validateEnv,
    }),
    LoggingModule.forRootAsync({
      inject: [ConfigService],
      useFactory: (config: ConfigService<Env, true>) => ({
        service: 'indexer-enrich',
        level: config.get('LOG_LEVEL', { infer: true }),
        pretty: config.get('LOG_PRETTY', { infer: true }),
      }),
    }),
    TokenEnrichmentModule.forRootAsync({
      imports: [ConfigModule],
      inject: [ConfigService],
      useFactory: (config: ConfigService<Env, true>) => ({
        chainId: config.get('CHAIN_ID', { infer: true }),
        sweepIntervalMs: 0,
        concurrency: config.get('TOKEN_ENRICHMENT_CONCURRENCY', { infer: true }),
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
        postgres: { url: config.get('POSTGRES_URL', { infer: true }) },
      }),
    }),
  ],
  providers: [TokenEnricher],
})
export class EnrichModule {}
