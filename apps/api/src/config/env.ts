import { LOG_LEVELS } from '@packages/ops';
import { z } from 'zod';

/**
 * Environment contract for the API process.
 *
 * Parsed once at boot. An invalid value aborts the process instead of
 * defaulting silently — a pod that crash-loops on bad config is far easier to
 * diagnose than one that runs against the wrong chain.
 */
export const envSchema = z.object({
  NODE_ENV: z.enum(['development', 'test', 'production']).default('development'),

  LOG_LEVEL: z.enum(LOG_LEVELS).default('info'),
  LOG_PRETTY: z
    .enum(['true', 'false', '1', '0'])
    .default('false')
    .transform((value) => value === 'true' || value === '1'),

  API_HOST: z.string().min(1).default('0.0.0.0'),
  API_PORT: z.coerce.number().int().min(1).max(65_535).default(3000),
  API_GLOBAL_PREFIX: z.string().default('api'),

  /**
   * Where Swagger UI and the raw OpenAPI document are mounted. Always served —
   * the docs are the API's contract, not a debug affordance. Sits outside the
   * global prefix, so this is a root-level path.
   */
  API_DOCS_PATH: z
    .string()
    .min(1)
    .default('docs')
    .transform((value) => value.replace(/^\/+|\/+$/g, '')),

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
