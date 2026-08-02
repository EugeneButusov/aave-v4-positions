import { diag, DiagConsoleLogger, DiagLogLevel } from '@opentelemetry/api';
import { logs } from '@opentelemetry/api-logs';
import { OTLPLogExporter } from '@opentelemetry/exporter-logs-otlp-proto';
import { OTLPMetricExporter } from '@opentelemetry/exporter-metrics-otlp-proto';
import { OTLPTraceExporter } from '@opentelemetry/exporter-trace-otlp-proto';
import { registerInstrumentations } from '@opentelemetry/instrumentation';
import { HttpInstrumentation } from '@opentelemetry/instrumentation-http';
import { NestInstrumentation } from '@opentelemetry/instrumentation-nestjs-core';
import { PinoInstrumentation } from '@opentelemetry/instrumentation-pino';
import { UndiciInstrumentation } from '@opentelemetry/instrumentation-undici';
import { defaultResource, resourceFromAttributes } from '@opentelemetry/resources';
import { BatchLogRecordProcessor, LoggerProvider } from '@opentelemetry/sdk-logs';
import { MeterProvider, PeriodicExportingMetricReader } from '@opentelemetry/sdk-metrics';
import { BatchSpanProcessor, NodeTracerProvider } from '@opentelemetry/sdk-trace-node';
import { ATTR_SERVICE_NAME } from '@opentelemetry/semantic-conventions';

import { HEALTH_PATHS, IGNORED_METHODS, METRIC_EXPORT_INTERVAL_MS } from './conventions';

/**
 * Starts the OpenTelemetry SDK, as a side effect of being required.
 *
 * **This module is a preload, not an import.** Every service launches with
 * `node --require @packages/telemetry/start …`, so that the require hooks the
 * instrumentations install are in place before Nest — or `pino`, or `http` —
 * is loaded for the first time.
 *
 * `--require` rather than a first-line `import` in each `main.ts`, even though
 * a first-line import would order correctly here (the build is CommonJS, and
 * `tsc` emits `require` calls in source order). Three reasons, in the order
 * they matter:
 *
 * - **It can be turned off without rebuilding.** The overhead numbers in the
 *   README are an A/B of one byte-identical build with and without the flag.
 *   Compiled into `main.js`, measuring the SDK's cost means a second image.
 * - Six CLI entry points would each need the same import.
 * - Every spec that imports an app module would start an SDK.
 *
 * Its cost, stated: someone running `node dist/main.js` by hand gets no
 * telemetry and no error. The mitigation is the boot line below, which names
 * the endpoint — its absence in the first few lines of a log stream is the
 * tell.
 *
 * **Not `@opentelemetry/sdk-node`.** See the catalog note in
 * `pnpm-workspace.yaml`: it would put 4.3 MB of gRPC in the image for an
 * HTTP exporter. The three providers are composed here instead.
 */

/**
 * `.env` is loaded here, not by Nest.
 *
 * `@nestjs/config` writes dotenv values into `process.env` only for keys that
 * are not already set, and only once the module graph is built — which is long
 * after this file has read the environment. Without this call an
 * `OTEL_EXPORTER_OTLP_ENDPOINT` in a local `.env` would be silently ignored
 * while every other variable in the same file worked, which is the worst
 * possible shape for a configuration bug.
 */
function loadLocalEnvFile(): void {
  try {
    process.loadEnvFile();
  } catch {
    // No `.env`. Every deployed environment injects variables directly, so
    // this is the normal case rather than a failure.
  }
}

function isTruthy(value: string | undefined): boolean {
  return value === 'true' || value === '1';
}

function start(): void {
  loadLocalEnvFile();

  if (isTruthy(process.env['OTEL_SDK_DISABLED'])) return;

  // The SDK's own diagnostics, off by default. An exporter that cannot reach
  // its endpoint is otherwise completely silent.
  if (process.env['OTEL_LOG_LEVEL'] === 'debug') {
    diag.setLogger(new DiagConsoleLogger(), DiagLogLevel.DEBUG);
  }

  const serviceName = process.env['OTEL_SERVICE_NAME'];
  if (serviceName === undefined || serviceName.length === 0) {
    // Refused rather than defaulted. Every signal is grouped by `service.name`,
    // so an unnamed service produces telemetry that is present, plausible and
    // impossible to attribute — and it would only be noticed in an incident.
    throw new Error('OTEL_SERVICE_NAME must be set when telemetry is enabled');
  }

  const resource = defaultResource().merge(
    resourceFromAttributes({ [ATTR_SERVICE_NAME]: serviceName }),
  );

  const tracerProvider = new NodeTracerProvider({
    resource,
    spanProcessors: [new BatchSpanProcessor(new OTLPTraceExporter())],
  });
  // Installs the AsyncLocalStorage context manager and the W3C propagators, so
  // an active span survives an `await` and `traceparent` is read and written on
  // every HTTP hop. The ClickHouse client's own tracer depends on the former.
  tracerProvider.register();

  const meterProvider = new MeterProvider({
    resource,
    readers: [
      new PeriodicExportingMetricReader({
        exporter: new OTLPMetricExporter(),
        // The SDK default is 60s. A gauge sampled once a minute cannot show a
        // backfill catching up, which is the thing these metrics exist for.
        exportIntervalMillis: METRIC_EXPORT_INTERVAL_MS,
      }),
    ],
  });

  const loggerProvider = new LoggerProvider({
    resource,
    processors: [new BatchLogRecordProcessor({ exporter: new OTLPLogExporter() })],
  });
  logs.setGlobalLoggerProvider(loggerProvider);

  registerInstrumentations({
    tracerProvider,
    meterProvider,
    instrumentations: [
      new HttpInstrumentation({
        // Probes fire every ten seconds against both services. Left in, they
        // are the overwhelming majority of every trace list — the same reason
        // `buildLoggingParams` excludes them from request logging.
        ignoreIncomingRequestHook: (request) =>
          HEALTH_PATHS.some((path) => request.url?.startsWith(path) ?? false),
        // The SDK already suppresses tracing inside exporter calls, so the
        // OTLP posts do not trace themselves. This only drops the noise of
        // preflight-style requests.
        ignoreOutgoingRequestHook: (options) =>
          IGNORED_METHODS.has((options.method ?? 'GET').toUpperCase()),
      }),
      // Node's global `fetch`, which is what viem's HTTP transport uses — so
      // this is what attributes an RPC call to a provider. A separate
      // instrumentation from the one above, because `@clickhouse/client` uses
      // `node:http` and viem does not.
      new UndiciInstrumentation(),
      new NestInstrumentation(),
      // Two jobs from one dependency: `trace_id`/`span_id` onto every line,
      // and a destination that ships records to the Logs SDK. stdout is
      // untouched — the JSON-per-line contract is what a deployment reads.
      new PinoInstrumentation(),
    ],
  });

  const shutdown = async (): Promise<void> => {
    // Every provider force-flushes on shutdown. Without this a CLI that runs
    // for less than the batch delay exports nothing at all.
    await Promise.allSettled([
      tracerProvider.shutdown(),
      meterProvider.shutdown(),
      loggerProvider.shutdown(),
    ]);
  };

  process.once('beforeExit', () => {
    void shutdown();
  });

  // Deliberately `console.warn`, not pino: this runs before any logger exists,
  // and its whole purpose is to be visible when someone has forgotten
  // `--require`. Hand-shaped as JSON so it does not break the log contract.
  console.warn(
    JSON.stringify({
      service: serviceName,
      msg: 'telemetry started',
      endpoint: process.env['OTEL_EXPORTER_OTLP_ENDPOINT'] ?? 'http://localhost:4318',
    }),
  );
}

start();
