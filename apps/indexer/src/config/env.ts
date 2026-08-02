import {
  CORE_HUB_ADDRESS,
  CORE_HUB_GENESIS_BLOCK,
  MAIN_SPOKE_ADDRESS,
  MAIN_SPOKE_ORACLE_ADDRESS,
} from '@aave-positions/events';
import { LOG_LEVELS } from '@packages/ops';
import { z } from 'zod';

/**
 * How to reach ClickHouse.
 *
 * Its own fragment because the migration entry point needs exactly these and
 * none of the chain configuration — it applies DDL and exits, so demanding a
 * `CHAIN_ID` from it would be asking for a value it has no use for.
 */
const clickHouseEnv = {
  CLICKHOUSE_URL: z.url().default('http://localhost:8123'),
  CLICKHOUSE_DATABASE: z.string().min(1).default('default'),
  CLICKHOUSE_USER: z.string().min(1).default('default'),
  // Empty is legitimate: a container started with CLICKHOUSE_SKIP_USER_SETUP
  // has no password, which is how the test and CI instances run.
  CLICKHOUSE_PASSWORD: z.string().default(''),
};

export const clickHouseEnvSchema = z.object(clickHouseEnv);

export type ClickHouseEnv = z.infer<typeof clickHouseEnvSchema>;

/**
 * How to reach Postgres, where the indexer keeps its own position.
 *
 * Its own fragment for the same reason as ClickHouse's: the migration entry
 * point applies DDL to both databases and needs nothing else.
 *
 * One URL rather than the four discrete variables ClickHouse gets. Every managed
 * Postgres hands you exactly this string, often carrying `?sslmode=require`, and
 * postgres.js takes one directly — splitting it into parts means reassembling
 * it, and the first thing reassembly gets wrong is percent-encoding a password.
 */
const postgresEnv = {
  POSTGRES_URL: z.url().default('postgres://postgres@localhost:5432/postgres'),
};

export const postgresEnvSchema = z.object(postgresEnv);

export type PostgresEnv = z.infer<typeof postgresEnvSchema>;

/**
 * Environment contract for the indexer process.
 *
 * Parsed once at boot. An invalid value aborts the process instead of
 * defaulting silently — an indexer pointed at the wrong chain or the wrong
 * Spoke produces plausible, wrong data rather than an error.
 */
export const envSchema = z.object({
  NODE_ENV: z.enum(['development', 'test', 'production']).default('development'),

  LOG_LEVEL: z.enum(LOG_LEVELS).default('info'),
  LOG_PRETTY: z
    .enum(['true', 'false', '1', '0'])
    .default('false')
    .transform((value) => value === 'true' || value === '1'),

  /**
   * The indexer is a worker, but it still serves an HTTP port so Kubernetes has
   * something to probe. No business endpoints are mounted here.
   */
  INDEXER_HOST: z.string().min(1).default('0.0.0.0'),
  INDEXER_PORT: z.coerce.number().int().min(1).max(65_535).default(3001),

  SHUTDOWN_GRACE_SECONDS: z.coerce.number().int().min(0).max(300).default(10),

  /**
   * Required, with no default. A default here would be the exact failure this
   * file exists to prevent: an indexer silently pointed at the wrong chain
   * produces plausible, wrong data. Checked against what the providers actually
   * report on the first iteration.
   */
  CHAIN_ID: z.coerce.number().int().positive(),

  /**
   * Comma-separated, tried in order — the first is preferred, not merely one of
   * several. Provider capability varies enough to matter: measured
   * `eth_getLogs` ranges across public endpoints run from 50 blocks to 10,000,
   * and some serve no history at all (analysis §8).
   */
  RPC_URLS: z
    .string()
    .transform((value) =>
      value
        .split(',')
        .map((url) => url.trim())
        .filter((url) => url.length > 0),
    )
    .pipe(z.array(z.url()).min(1)),

  /**
   * How deep a reorg is assumed to reach. Consumed by the reorg detector, never
   * by the loop. 128 is the upper end of the usual range and comfortably past
   * the 75 blocks by which `finalized` was measured to trail (analysis §9.3).
   */
  FINALITY_DEPTH: z.coerce.number().int().min(0).max(10_000).default(128),

  /**
   * The floor for every processor, used only when the cursor store has nothing.
   *
   * **Core Hub genesis, not the Main Spoke's** — the Hub's first log is eight
   * blocks earlier, and the floor has to be the earliest of the contracts being
   * followed or the later one starts mid-history. Those eight blocks happen to
   * hold only the Hub proxy's own lifecycle events, so the Spoke's genesis
   * would lose nothing today; depending on that is the problem, because the
   * first `AddAsset` is the sole source of `underlying` and `decimals` and
   * missing it looks like a quiet chain rather than an error.
   */
  INDEXER_START_BLOCK: z.coerce.number().int().min(0).default(CORE_HUB_GENESIS_BLOCK),

  /** Upper bound on one dispatched range; halved at runtime if a provider balks. */
  INDEXER_MAX_RANGE_SIZE: z.coerce.number().int().min(1).max(100_000).default(1_000),

  INDEXER_POLL_INTERVAL_MS: z.coerce.number().int().min(100).max(600_000).default(4_000),
  INDEXER_RPC_TIMEOUT_MS: z.coerce.number().int().min(100).max(120_000).default(10_000),

  /** How long the loop may fail to progress before readiness starts failing. */
  INDEXER_STALL_THRESHOLD_MS: z.coerce.number().int().min(1_000).default(300_000),

  /** Turned off in tests, so importing the module does not start indexing. */
  INDEXER_AUTOSTART: z
    .enum(['true', 'false', '1', '0'])
    .default('true')
    .transform((value) => value === 'true' || value === '1'),

  /**
   * Which Spoke to follow. Defaults to the Main Spoke on mainnet, but it is
   * configuration rather than a constant: the processor takes its address as an
   * option, so a second Spoke is a second registration.
   */
  MAIN_SPOKE_ADDRESS: z
    .string()
    .regex(/^0x[0-9a-fA-F]{40}$/, 'must be a 20-byte hex address')
    .default(MAIN_SPOKE_ADDRESS)
    .transform((value) => value.toLowerCase()),

  /**
   * The oracle that prices that Spoke's reserves.
   *
   * **Paired with the Spoke, not with the chain.** `IAaveOracleV4` belongs to
   * one Spoke and is keyed by `reserveId` (§7.4), so the two move together: a
   * second Spoke brings a second oracle over a disjoint id space, and asking
   * the wrong one about a reserve id gets a price back rather than an error.
   */
  MAIN_SPOKE_ORACLE_ADDRESS: z
    .string()
    .regex(/^0x[0-9a-fA-F]{40}$/, 'must be a 20-byte hex address')
    .default(MAIN_SPOKE_ORACLE_ADDRESS)
    .transform((value) => value.toLowerCase()),

  /**
   * Which Hub to follow, for the asset state that turns shares into balances
   * (§5). All 14 Main Spoke reserves point at the Core Hub, so this pair needs
   * no cross-hub read — but it is configuration for the same reason the Spoke
   * is: a second Hub is a second registration, not an edit.
   */
  CORE_HUB_ADDRESS: z
    .string()
    .regex(/^0x[0-9a-fA-F]{40}$/, 'must be a 20-byte hex address')
    .default(CORE_HUB_ADDRESS)
    .transform((value) => value.toLowerCase()),

  /**
   * How long enrichment waits before retrying after a run left a gap open.
   *
   * Only after a *failure*. A run that resolved everything imposes no delay, so
   * a newly listed token is picked up on the next dispatch rather than waiting
   * out a timer it did not earn. This is the backstop against a dead provider
   * being re-asked on every block.
   */
  TOKEN_ENRICHMENT_RETRY_MS: z.coerce.number().int().min(1_000).max(86_400_000).default(60_000),

  /**
   * How many tokens are read from the chain at once.
   *
   * Four, not unbounded. A public endpoint rate-limits a burst long before
   * seventeen calls become slow, and the transport is configured with
   * `retryCount: 0`, so one 429 under an unbounded fan-out fails the whole
   * sweep.
   */
  TOKEN_ENRICHMENT_CONCURRENCY: z.coerce.number().int().min(1).max(64).default(4),

  /**
   * How long a successful price read stays good before the next one.
   *
   * **Unlike enrichment's delay, this one applies after success.** A token read
   * once is never read again, so that timer only exists to pace retries; a price
   * is never finished, so this is the cadence itself. One `eth_call` covers the
   * whole Spoke, which makes the bound politeness towards the provider rather
   * than cost — and a minute is already far more often than the feeds move,
   * measured at roughly one `AnswerUpdated` an hour per aggregate (§7.4).
   */
  RESERVE_PRICE_REFRESH_MS: z.coerce.number().int().min(1_000).max(86_400_000).default(60_000),

  /**
   * How long to wait after a read that left a price stale.
   *
   * Shorter than the refresh, deliberately. A price standing still while its
   * neighbours moved is worse than one uniformly a minute old, because §7.1
   * weighs collateral against debt and only the *ratio* is wrong.
   */
  RESERVE_PRICE_RETRY_MS: z.coerce.number().int().min(1_000).max(86_400_000).default(15_000),

  ...clickHouseEnv,
  ...postgresEnv,
});

export type Env = z.infer<typeof envSchema>;

/** `validate` hook for `ConfigModule.forRoot`. */
export function validateEnv(raw: Record<string, unknown>): Env {
  const result = envSchema.safeParse(raw);
  if (!result.success) {
    throw new Error(`Invalid environment configuration:\n${z.prettifyError(result.error)}`);
  }
  return result.data;
}
