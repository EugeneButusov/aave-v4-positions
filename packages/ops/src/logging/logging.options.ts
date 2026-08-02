import { randomUUID } from 'node:crypto';
import type { IncomingMessage, ServerResponse } from 'node:http';

import { RequestMethod } from '@nestjs/common';
import { isSpanContextValid, trace } from '@opentelemetry/api';
import type { Params as PinoParams } from 'nestjs-pino';

export const REQUEST_ID_HEADER = 'x-request-id';

/**
 * The trace this request belongs to, or `null` when nothing is tracing.
 *
 * Returns `null` rather than throwing when no SDK is registered: the API's
 * whole logging setup must keep working with telemetry switched off, and it
 * does, because `@opentelemetry/api` is a no-op facade until something
 * registers a provider.
 */
function activeTraceId(): string | null {
  const context = trace.getActiveSpan()?.spanContext();
  return context !== undefined && isSpanContextValid(context) ? context.traceId : null;
}

export const LOG_LEVELS = ['fatal', 'error', 'warn', 'info', 'debug', 'trace', 'silent'] as const;
export type LogLevel = (typeof LOG_LEVELS)[number];

export interface LoggingOptions {
  /** Tags every line, so one log stream can carry several services. */
  service: string;
  level?: LogLevel;
  /**
   * Human-readable output. Local development only — anywhere else this must
   * stay off so each line remains a single parseable JSON object.
   */
  pretty?: boolean;
  /** Extra fields tagged onto every line, e.g. the indexer's `chainId`. */
  base?: Record<string, string | number>;
  /** Paths excluded from automatic request logging. Probes by default. */
  ignorePathPrefixes?: string[];
}

/**
 * Builds the nestjs-pino parameters shared by every service.
 *
 * Kept as a pure function so the behaviour that matters — request-id
 * propagation, probe filtering, redaction — is unit-testable without standing
 * up a Nest application.
 */
export function buildLoggingParams(options: LoggingOptions): PinoParams {
  const { service, level, pretty = false, base = {}, ignorePathPrefixes = ['/health'] } = options;

  return {
    // Express 5 rejects the bare `*` that nestjs-pino defaults to; the
    // named-wildcard form is the path-to-regexp v8 spelling of the same thing.
    forRoutes: [{ path: '{*path}', method: RequestMethod.ALL }],
    pinoHttp: {
      level,
      transport: pretty
        ? { target: 'pino-pretty', options: { singleLine: true, translateTime: 'SYS:HH:MM:ss.l' } }
        : undefined,
      base: { service, ...base },
      /**
       * Propagated from the caller when present, minted otherwise, and echoed
       * back on the response — that header is what makes a log line traceable
       * from a client-side report.
       *
       * **The active trace id is preferred over a fresh UUID.** By the time
       * this runs the HTTP instrumentation has already opened a server span, so
       * without this a line would carry a `reqId` and a `trace_id` that have
       * nothing to do with each other, and the echoed header would name neither
       * the trace nor anything findable. With it, the two agree.
       *
       * `x-request-id` still exists alongside `traceparent`, and deliberately:
       * it is caller-supplied, echoed on the response, and stable across a
       * retry, none of which is true of a trace id. They answer different
       * questions and every line carries both.
       */
      genReqId: (req: IncomingMessage, res: ServerResponse) => {
        const incoming = req.headers[REQUEST_ID_HEADER];
        const id =
          (Array.isArray(incoming) ? incoming[0] : incoming) ?? activeTraceId() ?? randomUUID();
        res.setHeader(REQUEST_ID_HEADER, id);
        return id;
      },
      // Probes fire every few seconds; logging them buries everything else.
      autoLogging: {
        ignore: (req: IncomingMessage) =>
          ignorePathPrefixes.some((prefix) => req.url?.startsWith(prefix) ?? false),
      },
      redact: {
        paths: ['req.headers.authorization', 'req.headers.cookie'],
        remove: true,
      },
    },
  };
}
