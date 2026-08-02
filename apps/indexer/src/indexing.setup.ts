import type { DynamicModule } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import {
  HUB_EVENT_PROCESSOR,
  HubEventsModule,
  SPOKE_EVENT_PROCESSOR,
  SpokeEventsModule,
} from '@aave-positions/events';
import {
  BLOCK_HEADER_STORE,
  HashChainReorgDetector,
  IndexingModule,
  PostgresBlockHeaderStore,
  PostgresCursorStore,
} from '@packages/indexing';
import { PostgresModule } from '@packages/postgres';

import type { Env } from './config/env';

/**
 * The indexing pipeline, in one place because two entry points need it
 * identical: the daemon in `AppModule`, and the backfill command.
 *
 * **The processor list is why this is shared rather than repeated.** A
 * processor added to one and forgotten in the other would not fail — a backfill
 * would quietly run fewer processors over the range and report success, which
 * is the worst shape this class of bug can take.
 *
 * Everything the pipeline needs comes back as **one** module. The event and
 * Postgres modules are built here and re-exported through it, so a caller
 * cannot hold one without the others or accidentally construct a second client
 * reaching for a health probe.
 *
 * **Call this once per process and use what it returns everywhere.** Nest
 * identifies a dynamic module by reference, so a second call is a second
 * pipeline, and with it a second `IndexerService` racing the same cursor. The
 * two roots each call it once, and never load together.
 */
export function indexingSetup(overrides: { readonly autoStart?: boolean } = {}): DynamicModule {
  /**
   * Everything Aave-specific: the client, the event log and the processor that
   * fills it. The application names which Spoke to follow and nothing else.
   */
  const spokeEvents = SpokeEventsModule.forRootAsync({
    imports: [ConfigModule],
    inject: [ConfigService],
    useFactory: (config: ConfigService<Env, true>) => ({
      chainId: config.get('CHAIN_ID', { infer: true }),
      spoke: config.get('MAIN_SPOKE_ADDRESS', { infer: true }),
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
    }),
  });

  /**
   * The Hub's asset state, which is what turns a share balance into a token
   * balance (§5). Its own module rather than an option on the Spoke's: two
   * contracts, two ABIs, two ledgers, and two independent "add another one"
   * stories.
   */
  const hubEvents = HubEventsModule.forRootAsync({
    imports: [ConfigModule],
    inject: [ConfigService],
    useFactory: (config: ConfigService<Env, true>) => ({
      chainId: config.get('CHAIN_ID', { infer: true }),
      hub: config.get('CORE_HUB_ADDRESS', { infer: true }),
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
    }),
  });

  /**
   * Where the indexer keeps its own position, separate from the events it
   * writes. Not the same database on purpose: the cursor and the header window
   * are a mutable set of about 130 rows rewritten on every iteration, which is
   * the one shape an append-only column store is bad at.
   */
  const postgres = PostgresModule.forRootAsync({
    imports: [ConfigModule],
    inject: [ConfigService],
    useFactory: (config: ConfigService<Env, true>) => ({
      url: config.get('POSTGRES_URL', { infer: true }),
    }),
  });

  return IndexingModule.forRootAsync({
    imports: [ConfigModule, spokeEvents, hubEvents, postgres],
    // Re-exported so the readiness probes both resolve through this module,
    // against the clients it already built. Only one of the two event modules
    // can be re-exported: both export ClickHouseHealthIndicator, and two
    // modules exporting one token into the same importer collide. They point at
    // the same server, so one probe answers for both.
    exports: [spokeEvents, postgres],
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
      // A command overrides this to `false`. Left to the environment otherwise,
      // where it is how a pod boots its probes without indexing.
      autoStart: overrides.autoStart ?? config.get('INDEXER_AUTOSTART', { infer: true }),
    }),
    processors: [SPOKE_EVENT_PROCESSOR, HUB_EVENT_PROCESSOR],
    reorgDetector: HashChainReorgDetector,
    cursorStore: PostgresCursorStore,
    providers: [
      HashChainReorgDetector,
      // Durable together, and in one database. The loop commits the header
      // before it saves the cursor and rewinds only after, so a crash between
      // the two leaves the window at or ahead of the cursor — the state
      // `bootstrap` is built to resolve. Making one of these durable without the
      // other would be worse than neither: a resume point nothing can vet turns
      // every cross-restart fork into `unrecoverable`.
      { provide: BLOCK_HEADER_STORE, useClass: PostgresBlockHeaderStore },
      PostgresCursorStore,
    ],
  });
}
