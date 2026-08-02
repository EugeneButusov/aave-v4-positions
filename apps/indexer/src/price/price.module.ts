import { Module } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import { ReservePriceModule } from '@packages/prices';
import { LoggingModule } from '@packages/ops';

import { validateEnv, type Env } from '../config/env';
import { ReservePricer } from './reserve-pricer';

/**
 * The one-shot pricing command's graph.
 *
 * Deliberately **not** `indexingSetup()`. That builds the cursor store, the
 * reorg detector and an `IndexerService` that would have to be remembered to
 * disable; this job needs a chain client and two databases, which is what
 * `ReservePriceModule` already assembles for the processor.
 *
 * The two delays are zero here: a command run by hand is always due, and the
 * operator watching it is the backoff. The processor in this graph is
 * constructed but never dispatched to, because nothing drives a loop.
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
        service: 'indexer-price',
        level: config.get('LOG_LEVEL', { infer: true }),
        pretty: config.get('LOG_PRETTY', { infer: true }),
      }),
    }),
    ReservePriceModule.forRootAsync({
      imports: [ConfigModule],
      inject: [ConfigService],
      useFactory: (config: ConfigService<Env, true>) => ({
        chainId: config.get('CHAIN_ID', { infer: true }),
        spoke: config.get('MAIN_SPOKE_ADDRESS', { infer: true }),
        oracle: config.get('MAIN_SPOKE_ORACLE_ADDRESS', { infer: true }),
        refreshMs: 0,
        retryMs: 0,
        // The command reads once and exits. Starting the module's timer would
        // give it a background poller it never asked for, and a process that
        // will not settle.
        autoStart: false,
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
  providers: [ReservePricer],
})
export class PriceModule {}
