import { Module } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import { ClickHouseHealthIndicator } from '@packages/clickhouse';
import { IndexerHealthIndicator } from '@packages/indexing';
import { HealthModule, LoggingModule } from '@packages/ops';
import { PostgresHealthIndicator } from '@packages/postgres';

import { PendingTokens } from '@aave-positions/enrichment';

import { validateEnv, type Env } from './config/env';
import { indexingSetup } from './indexing.setup';
import { metadataSetup } from './metadata.setup';
import { pricingSetup } from './pricing.setup';

/**
 * Built once and referenced twice below. Nest identifies a dynamic module by
 * object reference, so a second `indexingSetup()` call would produce a second
 * pipeline — and with it a second indexing loop racing the same cursor.
 */
/**
 * The handoff from ingestion to the metadata filler.
 *
 * Owned here because the two ends are in sibling modules and neither can reach
 * the other: dynamic-module exports flow outward to importers, not sideways.
 * The composition root is the only place that holds both, so it holds the thing
 * between them.
 */
const listedTokens = new PendingTokens();

const indexing = indexingSetup(listedTokens);

/**
 * **A peer of the pipeline, not a part of it.** Reading a third-party ERC-20
 * has no more to do with a block range than reading an oracle does — see
 * `TokenMetadataFiller` for the three things a block dispatch was silently
 * gating. Discovery still comes from the `AddAsset` the Hub processor decodes,
 * through the buffer above.
 */
const metadata = metadataSetup(listedTokens);

/**
 * **A peer of the pipeline, not a part of it.** Prices come from an oracle
 * whose feeds move off chain on their own schedule, so the refresh is driven by
 * wall-clock time rather than by a block dispatch — see `ReservePriceRefresher`
 * for what a `BlockProcessor` got wrong. Registered here rather than inside
 * `indexingSetup()` so a backfill, which is a one-shot over historical blocks,
 * does not acquire a background poller along with it.
 */
const pricing = pricingSetup();

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
    metadata,
    pricing,
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
