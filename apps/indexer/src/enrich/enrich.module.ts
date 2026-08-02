import { Module } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import { PendingTokens, TokenMetadataModule } from '@aave-positions/enrichment';
import { LoggingModule } from '@packages/ops';

import { validateEnv, type Env } from '../config/env';
import { TokenEnricher } from './token-enricher';

/**
 * The one-shot enrichment command's graph.
 *
 * Deliberately **not** `indexingSetup()`. That builds the cursor store, the
 * reorg detector and an `IndexerService` that would have to be remembered to
 * disable; this job needs a chain client and two databases, which is what
 * `TokenMetadataModule` already assembles for the filler.
 *
 * The retry delay is zero here: a command run by hand is always due, and the
 * operator watching it is the backoff.
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
    TokenMetadataModule.forRootAsync({
      imports: [ConfigModule],
      inject: [ConfigService],
      useFactory: (config: ConfigService<Env, true>) => ({
        chainId: config.get('CHAIN_ID', { infer: true }),
        // Its own, and always empty: nothing ingests during a one-shot run, so
        // the command works from the full listing set every time.
        pending: new PendingTokens(),
        retryDelayMs: 0,
        // Read once and exit. Starting the module's filler would give the
        // command a background worker it never asked for.
        autoStart: false,
        concurrency: config.get('TOKEN_METADATA_CONCURRENCY', { infer: true }),
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
