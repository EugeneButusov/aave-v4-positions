import type { DynamicModule } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import { TokenMetadataModule, type PendingTokens } from '@aave-positions/enrichment';

import type { Env } from './config/env';

/**
 * What each token calls itself, filled in automatically.
 *
 * **Its own setup rather than a branch of `indexingSetup()`**, for the reason
 * pricing has one: reading an ERC-20 has no more to do with a block range than
 * reading an oracle does. The loop was never the source of this work — the
 * event that lists a token is, and that arrives through `listedTokens` — it was
 * only an incidental clock, and one that stops when the indexer is caught up,
 * stalled, or not started.
 *
 * `listedTokens` is passed in rather than created here because the *writer* is
 * the Hub event processor, inside the pipeline. Dynamic-module exports flow
 * outward to importers and not sideways, so the composition root is the only
 * place that holds both ends.
 *
 * Keeping the two apart is also what stops a backfill acquiring a filler. That
 * command replays historical blocks and exits; the tokens it lists are pushed
 * into a buffer nothing in that process will drain, and the daemon owes a full
 * check on its next start regardless.
 *
 * **Call this once per process and use what it returns.** Nest identifies a
 * dynamic module by reference, so a second call is a second filler racing the
 * first through the same tokens.
 */
export function metadataSetup(listedTokens: PendingTokens): DynamicModule {
  return TokenMetadataModule.forRootAsync({
    imports: [ConfigModule],
    inject: [ConfigService],
    useFactory: (config: ConfigService<Env, true>) => ({
      chainId: config.get('CHAIN_ID', { infer: true }),
      pending: listedTokens,
      retryDelayMs: config.get('TOKEN_METADATA_RETRY_MS', { infer: true }),
      concurrency: config.get('TOKEN_METADATA_CONCURRENCY', { infer: true }),
      // Its own switch, not `INDEXER_AUTOSTART`. That one says whether to walk
      // the chain; this says whether to read third-party ERC-20s, and the whole
      // reason the filler left the loop is that those are separate questions.
      autoStart: config.get('TOKEN_METADATA_AUTOSTART', { infer: true }),
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
