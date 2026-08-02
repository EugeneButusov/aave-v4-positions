/**
 * The handful of values `start.ts` and the recording side both need.
 *
 * Its own file because `start.ts` is a preload with side effects: importing it
 * to reach a constant would start an SDK.
 */

/** Excluded from server spans, for the reason probes are excluded from logs. */
export const HEALTH_PATHS = ['/health/live', '/health/ready'] as const;

/** Never worth a client span, and noisy on every HTTP client in the process. */
export const IGNORED_METHODS = new Set(['OPTIONS', 'HEAD']);

/**
 * How often metrics are exported.
 *
 * Fifteen seconds, against the SDK's default of sixty. The gauges here exist to
 * show a backfill catching up and a provider failing over, and both are over
 * inside a minute — sampled at the default, `indexer.lag.blocks` would show a
 * flat line and then zero.
 */
export const METRIC_EXPORT_INTERVAL_MS = 15_000;

/**
 * One instrumentation scope for everything this repo records by hand.
 *
 * Distinct from the auto-instrumentation scopes, so a query can separate spans
 * we chose to create from spans a library created for us.
 */
export const SCOPE = 'aave-v4-positions';
