import { Logger } from '@nestjs/common';
import { metrics, trace, type Counter, type Histogram } from '@opentelemetry/api';

/**
 * Who is actually answering, and who has stopped.
 *
 * The gap this closes is specific. {@link connect} builds
 * `fallback(urls, { rank: false })`, and `rank: false` is deliberate — the list
 * is a *preference order*, not a set of interchangeable nodes. That makes the
 * failure mode silent: **when the preferred provider dies, everything is served
 * by the next one and nothing says so.** Before this, no request was attributed
 * to a URL, no 429 was counted, and a failover left no trace at all.
 *
 * The seam is viem's `fetchFn`, one per `http()` transport, so the URL is in
 * hand — which is exactly what `fallback` hides from everything above it.
 *
 * **`fetchFn` and not the `onFetchRequest`/`onFetchResponse` hooks**, which look
 * like the obvious choice and are not. They fire at two unconnected moments
 * with no handle tying them together, so neither a duration nor a
 * request-to-response attribute can be recovered from them — and neither sees a
 * *throw*, which is what a timeout or a refused connection actually is.
 * Wrapping the call sees all three.
 *
 * It is also the only place the JSON-RPC method is visible: every call is
 * `POST /` with the method in the body, so the undici instrumentation can only
 * ever report `POST`, and without the attribute added here every RPC span and
 * duration bucket collapses into one meaningless series.
 */

class ProviderInstruments {
  readonly requests: Counter;
  readonly errors: Counter;
  readonly failovers: Counter;
  readonly duration: Histogram;

  constructor() {
    const meter = metrics.getMeter('@packages/indexing');
    this.requests = meter.createCounter('rpc.client.requests', {
      description: 'RPC calls by provider, method and status. Which provider is answering.',
    });
    this.errors = meter.createCounter('rpc.client.errors', {
      description: 'Timeouts, 429s and 5xx kept apart, by provider.',
    });
    this.failovers = meter.createCounter('rpc.client.failovers', {
      description:
        'A call served by a provider other than the preferred one — the signal that had no way out.',
    });
    this.duration = meter.createHistogram('rpc.client.duration', {
      unit: 's',
      description: 'Per provider and per JSON-RPC method, which the URL alone cannot tell you.',
    });
  }
}

/** Built once and lazily, so importing this module registers no instruments. */
let instruments: ProviderInstruments | undefined;

function shared(): ProviderInstruments {
  instruments ??= new ProviderInstruments();
  return instruments;
}

const logger = new Logger('RpcProviders');

/**
 * Which provider last answered, so a failover is logged **once, on the
 * transition**.
 *
 * Not per request, and that is the whole reason this state exists: a dead
 * preferred provider is retried on every single call, so a line each would be
 * the entire log stream within a minute of an outage — the failure would bury
 * its own evidence.
 *
 * Module-level rather than per-transport, because three transports are built
 * per indexer process (the loop, each event module, enrichment) over the same
 * ordered URL list. They fail over together, and three copies of one transition
 * would read as three separate incidents.
 */
let serving: string | null = null;

/** Only the host is kept: an API key in a provider URL must not reach a label. */
export function providerLabel(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return 'invalid-url';
  }
}

/** `eth_getLogs` and friends live in the POST body, never in the URL. */
export function methodOf(body: unknown): string {
  if (typeof body !== 'string') return 'unknown';
  try {
    const parsed: unknown = JSON.parse(body);
    // Batched calls are one HTTP request carrying many methods. Naming it after
    // the first would be a lie the graph could not be talked out of.
    if (Array.isArray(parsed)) return 'batch';
    if (typeof parsed !== 'object' || parsed === null || !('method' in parsed)) return 'unknown';
    return typeof parsed.method === 'string' ? parsed.method : 'unknown';
  } catch {
    return 'unknown';
  }
}

type FetchFn = (input: string | URL | Request, init?: RequestInit) => Promise<Response>;

/**
 * Wraps `fetch` for one provider, closing over its URL and its place in the
 * preference order.
 *
 * `preferred` is `index === 0`. Anything else being asked at all means the one
 * before it did not answer.
 */
export function measuredFetch(url: string, index: number): FetchFn {
  const provider = providerLabel(url);
  const preferred = index === 0;

  return async (input, init) => {
    const method = methodOf(init?.body);
    const started = performance.now();
    const attributes = { provider, preferred, 'rpc.method': method };

    trace.getActiveSpan()?.setAttributes({ 'rpc.method': method, 'rpc.provider': provider });

    try {
      const response = await fetch(input, init);

      shared().duration.record((performance.now() - started) / 1000, attributes);
      shared().requests.add(1, { ...attributes, 'http.status_code': response.status });

      if (response.ok) {
        observeServing(provider, preferred);
      } else {
        // A 429 is the one everybody hits first: the transport is configured
        // with `retryCount: 0`, so a rate limit is a hard failure here rather
        // than something viem quietly rides out.
        shared().errors.add(1, { ...attributes, 'error.type': String(response.status) });
      }

      return response;
    } catch (error) {
      shared().duration.record((performance.now() - started) / 1000, attributes);
      shared().errors.add(1, {
        ...attributes,
        'error.type': error instanceof Error ? error.name : 'unknown',
      });
      throw error;
    }
  };
}

/**
 * Records who is serving, and logs only when that changes.
 *
 * Called on success only. A provider that errors has not taken over — the next
 * one in the list is about to be tried, and calling this on the way past would
 * report a failover to a node that never answered.
 */
export function observeServing(provider: string, preferred: boolean): void {
  if (serving === provider) return;

  const previous = serving;
  serving = provider;

  // The first successful call establishes the baseline. It is not a failover:
  // nothing has changed yet, and counting it would put a step on the graph at
  // every process start.
  if (previous === null) return;

  if (preferred) {
    logger.log({ provider, previousProvider: previous }, 'rpc provider recovered');
    return;
  }

  shared().failovers.add(1, { provider, previousProvider: previous });
  logger.warn({ provider, previousProvider: previous }, 'rpc provider failover');
}

/**
 * Test seam. Clears **both** pieces of module-level state, and the second one
 * is the non-obvious half.
 *
 * `instruments` is memoised so that importing this module registers nothing and
 * a request does not rebuild four instruments. In a process that is exactly
 * right — one meter provider for its lifetime. In a spec file it is a trap: the
 * first test to make a request binds the instruments to *that* test's provider,
 * and every later test then records into a provider that has been shut down,
 * so its assertions read an empty export and the failure looks like the code
 * under test.
 */
export function resetProviderHealth(): void {
  serving = null;
  instruments = undefined;
}
