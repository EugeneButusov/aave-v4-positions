import { Logger } from '@nestjs/common';
import { metrics } from '@opentelemetry/api';
import {
  AggregationTemporality,
  InMemoryMetricExporter,
  MeterProvider,
  PeriodicExportingMetricReader,
  type DataPoint,
} from '@opentelemetry/sdk-metrics';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { measuredFetch, methodOf, providerLabel, resetProviderHealth } from './provider-health';

const PREFERRED = 'https://preferred.example/rpc';
const SECONDARY = 'https://secondary.example/rpc';

let exporter: InMemoryMetricExporter;
let reader: PeriodicExportingMetricReader;
let provider: MeterProvider;

/** One JSON-RPC call, shaped the way viem stringifies it into `init.body`. */
function call(method: string): RequestInit {
  return { method: 'POST', body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params: [] }) };
}

function ok(): Response {
  return new Response('{"jsonrpc":"2.0","id":1,"result":"0x1"}', { status: 200 });
}

beforeEach(() => {
  resetProviderHealth();
  exporter = new InMemoryMetricExporter(AggregationTemporality.CUMULATIVE);
  reader = new PeriodicExportingMetricReader({ exporter, exportIntervalMillis: 600_000 });
  provider = new MeterProvider({ readers: [reader] });
  metrics.setGlobalMeterProvider(provider);
});

afterEach(async () => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  await provider.shutdown();
  metrics.disable();
});

async function points(name: string): Promise<DataPoint<number>[]> {
  await reader.forceFlush();
  return (
    exporter
      .getMetrics()
      .at(-1)
      ?.scopeMetrics.flatMap((scope) => scope.metrics)
      .filter((metric) => metric.descriptor.name === name)
      .flatMap((metric) => metric.dataPoints as DataPoint<number>[]) ?? []
  );
}

describe('provider health', () => {
  describe('the JSON-RPC method', () => {
    it('is read from the body, because the URL cannot carry it', () => {
      // Every call is `POST /`. Without this the undici instrumentation reports
      // `POST` for eth_getLogs and eth_chainId alike, and every duration bucket
      // is one series.
      expect(methodOf(JSON.stringify({ method: 'eth_getLogs' }))).toBe('eth_getLogs');
    });

    it('calls a batch a batch rather than naming it after the first call', () => {
      const body = JSON.stringify([{ method: 'eth_chainId' }, { method: 'eth_getLogs' }]);

      expect(methodOf(body)).toBe('batch');
    });

    it('degrades to unknown rather than throwing on a body it cannot read', () => {
      expect(methodOf('not json')).toBe('unknown');
      expect(methodOf(undefined)).toBe('unknown');
    });
  });

  it('labels a provider by host, so an API key in the URL never reaches a metric', () => {
    expect(providerLabel('https://eth.example/v2/SECRET-KEY')).toBe('eth.example');
  });

  describe('failover', () => {
    it('counts and logs when a secondary takes over, and says so only once', async () => {
      const warn = vi.spyOn(Logger.prototype, 'warn').mockImplementation(() => undefined);
      vi.stubGlobal('fetch', vi.fn().mockResolvedValue(ok()));

      // The preferred provider answers first, establishing the baseline.
      await measuredFetch(PREFERRED, 0)(PREFERRED, call('eth_chainId'));
      expect(warn).not.toHaveBeenCalled();

      // It dies; the secondary carries the next three calls.
      const secondary = measuredFetch(SECONDARY, 1);
      await secondary(SECONDARY, call('eth_getLogs'));
      await secondary(SECONDARY, call('eth_getLogs'));
      await secondary(SECONDARY, call('eth_getLogs'));

      // One line, not three — and not three hundred, which is what a line per
      // request would be within a minute of a real outage.
      expect(warn).toHaveBeenCalledTimes(1);
      expect(warn.mock.calls[0]?.[0]).toMatchObject({
        provider: 'secondary.example',
        previousProvider: 'preferred.example',
      });

      const failovers = await points('rpc.client.failovers');
      expect(failovers).toHaveLength(1);
      expect(failovers[0]?.value).toBe(1);
    });

    it('does not report the first successful call as a failover', async () => {
      const warn = vi.spyOn(Logger.prototype, 'warn').mockImplementation(() => undefined);
      vi.stubGlobal('fetch', vi.fn().mockResolvedValue(ok()));

      // A process that starts while the preferred provider is already down
      // begins on the secondary. Nothing has changed yet, and counting it would
      // put a step on the graph at every restart.
      await measuredFetch(SECONDARY, 1)(SECONDARY, call('eth_chainId'));

      expect(warn).not.toHaveBeenCalled();
      expect(await points('rpc.client.failovers')).toHaveLength(0);
    });

    it('logs the recovery when the preferred provider answers again', async () => {
      vi.spyOn(Logger.prototype, 'warn').mockImplementation(() => undefined);
      const log = vi.spyOn(Logger.prototype, 'log').mockImplementation(() => undefined);
      vi.stubGlobal('fetch', vi.fn().mockResolvedValue(ok()));

      await measuredFetch(PREFERRED, 0)(PREFERRED, call('eth_chainId'));
      await measuredFetch(SECONDARY, 1)(SECONDARY, call('eth_chainId'));
      await measuredFetch(PREFERRED, 0)(PREFERRED, call('eth_chainId'));

      expect(log).toHaveBeenCalledTimes(1);
      expect(log.mock.calls[0]?.[0]).toMatchObject({ provider: 'preferred.example' });
    });

    it('does not hand over to a provider that only errored', async () => {
      vi.spyOn(Logger.prototype, 'warn').mockImplementation(() => undefined);
      vi.stubGlobal('fetch', vi.fn().mockResolvedValue(ok()));
      await measuredFetch(PREFERRED, 0)(PREFERRED, call('eth_chainId'));

      // 429 is the one a public endpoint produces first, and `retryCount: 0`
      // makes it a hard failure rather than something viem rides out.
      vi.stubGlobal(
        'fetch',
        vi.fn().mockResolvedValue(new Response('rate limited', { status: 429 })),
      );
      await measuredFetch(SECONDARY, 1)(SECONDARY, call('eth_getLogs'));

      // It never answered, so it never took over.
      expect(await points('rpc.client.failovers')).toHaveLength(0);

      const errors = await points('rpc.client.errors');
      expect(errors[0]?.attributes).toMatchObject({
        provider: 'secondary.example',
        'error.type': '429',
      });
    });
  });

  it('counts a thrown request as an error and still rethrows', async () => {
    const timeout = Object.assign(new Error('timed out'), { name: 'TimeoutError' });
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(timeout));

    // A timeout and a refused connection are throws, not responses — which is
    // the whole reason this wraps the call rather than using viem's
    // request/response hooks, neither of which ever sees one.
    await expect(measuredFetch(PREFERRED, 0)(PREFERRED, call('eth_getLogs'))).rejects.toThrow(
      'timed out',
    );

    const errors = await points('rpc.client.errors');
    expect(errors[0]?.attributes).toMatchObject({
      provider: 'preferred.example',
      'error.type': 'TimeoutError',
      'rpc.method': 'eth_getLogs',
    });
  });

  it('attributes a request to its provider and method', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(ok()));

    await measuredFetch(PREFERRED, 0)(PREFERRED, call('eth_getLogs'));

    const requests = await points('rpc.client.requests');
    expect(requests[0]?.attributes).toMatchObject({
      provider: 'preferred.example',
      preferred: true,
      'rpc.method': 'eth_getLogs',
      'http.status_code': 200,
    });
    expect(await points('rpc.client.duration')).toHaveLength(1);
  });
});
