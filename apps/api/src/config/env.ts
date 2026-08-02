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

  /**
   * Telemetry, and a boundary worth being explicit about.
   *
   * **The SDK reads these from `process.env` itself, before this schema runs.**
   * It has to: `@packages/telemetry/start` is preloaded with `node --require`,
   * so it executes before Nest exists, let alone `ConfigModule`. Declaring them
   * here does not change where the SDK gets them from.
   *
   * They are declared anyway, for two things validation still buys: a malformed
   * endpoint aborts the process rather than being silently dropped by an
   * exporter, and the contract appears in one place with every other variable
   * instead of only in a Dockerfile. Standard `OTEL_*` names, no repo-invented
   * spelling — an operator who knows OpenTelemetry should not have to learn
   * ours.
   */
  OTEL_SDK_DISABLED: z
    .enum(['true', 'false', '1', '0'])
    .default('false')
    .transform((value) => value === 'true' || value === '1'),

  /**
   * Required whenever telemetry is on, and `start.ts` refuses to boot without
   * it. Every signal is grouped by `service.name`; an unnamed service produces
   * telemetry that is present, plausible and impossible to attribute — and that
   * is only ever noticed during an incident.
   */
  OTEL_SERVICE_NAME: z.string().min(1).optional(),

  OTEL_EXPORTER_OTLP_ENDPOINT: z.url().default('http://localhost:4318'),

  /** Everything is sampled by default. A single indexer is not a traffic problem. */
  OTEL_TRACES_SAMPLER: z.string().min(1).default('parentbased_always_on'),
  OTEL_TRACES_SAMPLER_ARG: z.string().optional(),

  /**
   * Where the fold lives.
   *
   * Copied from the indexer's contract rather than shared with it. These are
   * two independently deployable services, and hoisting the fragment into
   * `@packages/clickhouse` would put a validation library in the package that
   * owns the client — a cost paid by every consumer to save four lines here.
   */
  CLICKHOUSE_URL: z.url().default('http://localhost:8123'),
  CLICKHOUSE_DATABASE: z.string().min(1).default('default'),
  CLICKHOUSE_USER: z.string().min(1).default('default'),
  // Empty is legitimate: a container started with CLICKHOUSE_SKIP_USER_SETUP
  // has no password, which is how the test and CI instances run.
  CLICKHOUSE_PASSWORD: z.string().default(''),

  /**
   * Where the indexer records how far it got. Read-only from here — the API
   * stamps every response with it and writes nothing.
   */
  POSTGRES_URL: z.url().default('postgres://postgres@localhost:5432/postgres'),

  /**
   * Signs pagination cursors, so one cannot be altered or carried to a
   * different listing.
   *
   * Required, with no default: a default would be a key every deployment
   * shares, and a shared key is not a signature. **Must be identical across
   * replicas** — each process signs with its own copy, so a per-process secret
   * means a cursor issued by one pod is rejected by the next, which shows up as
   * pagination that fails only under load and only sometimes.
   */
  POSITIONS_CURSOR_SECRET: z.string().min(32),

  /**
   * Above this, a response reports itself stale.
   *
   * Deliberately *not* the indexer's `INDEXER_STALL_THRESHOLD_MS` (300s). That
   * one decides whether to drain traffic from a pod; this one tells a reader
   * their numbers are a minute old. Different questions, and a reader wants to
   * know long before an operator does.
   */
  API_SYNC_STALE_AFTER_SECONDS: z.coerce.number().int().min(1).default(60),

  /**
   * Above this, a page reports its prices stale.
   *
   * **It measures how long since we last read the oracle, never how long since
   * a feed last moved.** That distinction is the trap §7.5 names: an hour
   * without an `AnswerUpdated` is ordinary Chainlink behaviour, so a threshold
   * derived from feed cadence would mark healthy feeds stale forever and teach
   * readers to ignore the flag.
   *
   * Five times the indexer's `RESERVE_PRICE_REFRESH_MS` default, so one missed
   * refresh is not an alarm and a dead poller still surfaces quickly. Longer
   * than `API_SYNC_STALE_AFTER_SECONDS` because the two clocks run at different
   * rates: the indexer advances every block, the oracle is read every minute.
   */
  API_PRICE_STALE_AFTER_SECONDS: z.coerce.number().int().min(1).default(300),
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
