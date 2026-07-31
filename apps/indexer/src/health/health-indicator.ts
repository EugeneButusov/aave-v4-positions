/**
 * Extension point for readiness. A dependency (database, RPC provider, cache)
 * contributes one indicator; the readiness endpoint aggregates them.
 */
export interface HealthIndicator {
  readonly name: string;
  check(): Promise<void> | void;
}

/** Multi-provider token. Bind with `{ provide: HEALTH_INDICATORS, useValue: [...] }`. */
export const HEALTH_INDICATORS = Symbol('HEALTH_INDICATORS');

export type CheckStatus = 'up' | 'down';

export interface CheckResult {
  name: string;
  status: CheckStatus;
  error?: string;
}

export interface ReadinessReport {
  status: 'ok' | 'degraded' | 'shutting_down';
  checks: CheckResult[];
}

export interface LivenessReport {
  status: 'ok';
  uptimeSeconds: number;
}
