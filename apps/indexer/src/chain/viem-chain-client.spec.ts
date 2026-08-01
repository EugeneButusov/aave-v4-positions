import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { ViemChainClient } from './viem-chain-client';

interface RpcCall {
  readonly url: string;
  readonly method: string;
}

/** A JSON-RPC 200 carrying `result`. */
function ok(result: unknown): Response {
  return new Response(JSON.stringify({ jsonrpc: '2.0', id: 1, result }), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
}

/**
 * Stubs global fetch — which is what viem's http transport calls — and records
 * every request so tests can assert on provider order and on how many times a
 * method actually reached the network.
 */
function stubRpc(handler: (call: RpcCall) => Response | Promise<Response>): RpcCall[] {
  const calls: RpcCall[] = [];

  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
      const body: unknown = JSON.parse(typeof init?.body === 'string' ? init.body : '{}');
      const method =
        typeof body === 'object' && body !== null && 'method' in body ? String(body.method) : '';
      // fetch normalises a bare origin to a trailing slash; strip it so tests
      // can compare against the URLs as they were configured.
      const href = input instanceof Request ? input.url : input.toString();
      const call = { url: href.replace(/\/$/, ''), method };
      calls.push(call);
      return handler(call);
    }),
  );

  return calls;
}

const PRIMARY = 'https://primary.invalid';
const SECONDARY = 'https://secondary.invalid';

function build(urls: string[] = [PRIMARY]): ViemChainClient {
  return new ViemChainClient({ rpcUrls: urls, rpcTimeoutMs: 1_000 });
}

describe('ViemChainClient', () => {
  beforeEach(() => {
    vi.unstubAllGlobals();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('builds its client without reaching the network', () => {
    const calls = stubRpc(() => ok('0x1'));

    build([PRIMARY, SECONDARY]);

    // Boot must not depend on a reachable node: the pod comes up and reports
    // not-ready rather than crash-looping while the provider is down.
    expect(calls).toEqual([]);
  });

  it('reads the chain id as a number', async () => {
    stubRpc(() => ok('0x1'));

    await expect(build().getChainId()).resolves.toBe(1);
  });

  it('converts the head block number from bigint to number', async () => {
    stubRpc(() => ok('0x1793603'));

    // 0x1793603 is the Main Spoke genesis block. bigint stops at this boundary
    // and nothing downstream sees one.
    const head = await build().getHeadBlockNumber();

    expect(head).toBe(24_720_899);
    expect(typeof head).toBe('number');
  });

  it('converts a block header, dropping every bigint', async () => {
    stubRpc(() =>
      ok({
        number: '0x64',
        hash: `0x${'a'.repeat(64)}`,
        parentHash: `0x${'b'.repeat(64)}`,
        timestamp: '0x6600',
        transactions: [],
      }),
    );

    const header = await build().getBlockHeader(100);

    expect(header).toEqual({
      number: 100,
      hash: `0x${'a'.repeat(64)}`,
      parentHash: `0x${'b'.repeat(64)}`,
      timestamp: 0x6600,
    });
  });

  it('falls over to the next provider when the first fails', async () => {
    const calls = stubRpc(({ url }) => {
      if (url === PRIMARY) throw new Error('connection refused');
      return ok('0x64');
    });

    await expect(build([PRIMARY, SECONDARY]).getHeadBlockNumber()).resolves.toBe(100);

    expect(calls.map((c) => c.url)).toEqual([PRIMARY, SECONDARY]);
  });

  it('uses the first provider while it is healthy', async () => {
    const calls = stubRpc(() => ok('0x64'));

    await build([PRIMARY, SECONDARY]).getHeadBlockNumber();

    // `rank: false`. The list is a preference order, not an interchangeable pool.
    expect(calls.map((c) => c.url)).toEqual([PRIMARY]);
  });

  it('gives a failing provider exactly one attempt before falling over', async () => {
    const calls = stubRpc(({ url }) => {
      // A retryable 5xx rather than a thrown error: viem treats a hard
      // connection failure as non-retryable regardless, so only a retryable
      // status could expose a per-provider retry if one existed.
      if (url === PRIMARY) return new Response('upstream error', { status: 500 });
      return ok('0x64');
    });

    await expect(build([PRIMARY, SECONDARY]).getHeadBlockNumber()).resolves.toBe(100);

    expect(calls.filter((c) => c.url === PRIMARY)).toHaveLength(1);
  });

  it('re-reads the head every call instead of serving a cached value', async () => {
    let height = 0x64;
    stubRpc(() => ok(`0x${(height++).toString(16)}`));

    const client = build();

    // viem caches eth_blockNumber for `cacheTime`, which defaults to the 4s
    // polling interval. Cached, the loop would idle through blocks it could
    // have been indexing.
    await expect(client.getHeadBlockNumber()).resolves.toBe(100);
    await expect(client.getHeadBlockNumber()).resolves.toBe(101);
  });

  it('surfaces a total outage immediately instead of re-sweeping the provider list', async () => {
    const calls = stubRpc(() => new Response('upstream error', { status: 500 }));

    await expect(build([PRIMARY, SECONDARY]).getHeadBlockNumber()).rejects.toThrow(
      /HTTP request failed/,
    );

    // One attempt per provider, then give up. `fallback`'s own retryCount
    // defaults to 3, which re-runs the whole list four times over before the
    // caller hears anything — delay the loop cannot see and cannot factor into
    // its own backoff.
    expect(calls).toHaveLength(2);
  });
});
