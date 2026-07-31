import { LOG_LEVELS } from '@aave-v4-positions/platform';
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
