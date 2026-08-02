import type { DynamicModule } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import { ReservePriceModule } from '@packages/prices';

import type { Env } from './config/env';

/**
 * What Aave prices each reserve at, refreshed on a timer.
 *
 * **Its own setup rather than a branch of `indexingSetup()`**, because it is
 * not part of the pipeline. Enrichment is: a token's metadata changes on chain,
 * so the event that changes it is a sound trigger and the loop can carry it. An
 * oracle's feeds move off chain on their own schedule and no Aave event
 * announces one, so this is driven by wall-clock time and shares nothing with
 * the loop's cadence.
 *
 * Keeping the two apart is also what stops a backfill acquiring a poller. That
 * command replays historical blocks and exits; a price is a current-value
 * question, so there is nothing for it to do there.
 *
 * **Call this once per process and use what it returns.** Nest identifies a
 * dynamic module by reference, so a second call is a second refresher — two
 * timers reading the same oracle and writing the same rows.
 */
export function pricingSetup(): DynamicModule {
  return ReservePriceModule.forRootAsync({
    imports: [ConfigModule],
    inject: [ConfigService],
    useFactory: (config: ConfigService<Env, true>) => ({
      chainId: config.get('CHAIN_ID', { infer: true }),
      // The Spoke and its oracle travel together: `IAaveOracleV4` is per-Spoke
      // and keyed by `reserveId` (§7.4), so a reserve id means nothing without
      // knowing which contract to ask.
      spoke: config.get('MAIN_SPOKE_ADDRESS', { infer: true }),
      oracle: config.get('MAIN_SPOKE_ORACLE_ADDRESS', { infer: true }),
      refreshMs: config.get('RESERVE_PRICE_REFRESH_MS', { infer: true }),
      retryMs: config.get('RESERVE_PRICE_RETRY_MS', { infer: true }),
      // **Deliberately its own switch, not `INDEXER_AUTOSTART`.** That one says
      // whether to walk the chain; this says whether to poll an oracle, and the
      // whole reason the refresher left the loop is that those are separate
      // questions with separate failure modes. A pod serving probes while the
      // indexer is stopped should still serve prices that move.
      autoStart: config.get('RESERVE_PRICE_AUTOSTART', { infer: true }),
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
  });
}
