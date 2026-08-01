import { LOG_LEVELS } from '@aave-v4-positions/ops';
import { z } from 'zod';

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

  /** Main Spoke genesis. Used only when the cursor store has nothing. */
  INDEXER_START_BLOCK: z.coerce.number().int().min(0).default(24_720_899),

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
