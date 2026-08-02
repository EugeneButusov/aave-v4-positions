/**
 * What the rest of the repo records against.
 *
 * Small on purpose, and importing it starts nothing: the SDK lives behind
 * `@packages/telemetry/start`, which is preloaded with `node --require`. With
 * no SDK registered, everything here is the OpenTelemetry API's own no-op —
 * one allocation per span and no export — which is what makes it safe to call
 * from a hot loop and from a spec.
 *
 * There is no Nest module here, and that is not an omission. `@opentelemetry/api`
 * resolves its providers through a global registered at preload time, so
 * injecting a tracer would be indirection over something already reachable —
 * and a span has to *wrap* the work, so it could not be delivered by an
 * injected observer that is told what already happened.
 */

import { metrics, trace, type Meter, type Tracer } from '@opentelemetry/api';

import { SCOPE } from './conventions';

/** The tracer everything hand-instrumented in this repo uses. */
export function tracer(): Tracer {
  return trace.getTracer(SCOPE);
}

/** The meter everything hand-instrumented in this repo uses. */
export function meter(): Meter {
  return metrics.getMeter(SCOPE);
}

export { METRIC_EXPORT_INTERVAL_MS, SCOPE } from './conventions';
