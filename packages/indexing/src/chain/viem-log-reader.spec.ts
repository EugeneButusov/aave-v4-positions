import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { LogRangeTooLargeError } from './log-reader';
import { ViemLogReader } from './viem-log-reader';

interface RpcCall {
  readonly method: string;
  readonly params: unknown;
}

function ok(result: unknown): Response {
  return new Response(JSON.stringify({ jsonrpc: '2.0', id: 1, result }), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
}

/**
 * A JSON-RPC error carried on an HTTP 200, which is how three of the four
 * measured providers reject an over-wide range. viem lands the `message` on
 * `error.details`.
 */
function rpcError(code: number, message: string): Response {
  return new Response(JSON.stringify({ jsonrpc: '2.0', id: 1, error: { code, message } }), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
}

function stubRpc(handler: (call: RpcCall) => Response | Promise<Response>): RpcCall[] {
  const calls: RpcCall[] = [];

  vi.stubGlobal(
    'fetch',
    vi.fn(async (_input: string | URL | Request, init?: RequestInit) => {
      const body: unknown = JSON.parse(typeof init?.body === 'string' ? init.body : '{}');
      const call =
        typeof body === 'object' && body !== null && 'method' in body
          ? { method: String(body.method), params: (body as { params?: unknown }).params }
          : { method: '', params: undefined };
      calls.push(call);
      return handler(call);
    }),
  );

  return calls;
}

const SUPPLY = `0x${'d9'.repeat(32)}` as const;
const WITHDRAW = `0x${'fe'.repeat(32)}` as const;
const SPOKE = '0x94e7a5dcbe816e498b89ab752661904e2f56c485' as const;

function build(): ViemLogReader {
  return new ViemLogReader({ rpcUrls: ['https://provider.invalid'], rpcTimeoutMs: 1_000 });
}

function rpcLog(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    address: SPOKE,
    topics: [SUPPLY],
    data: '0x00',
    blockNumber: '0x64',
    blockHash: `0x${'a'.repeat(64)}`,
    blockTimestamp: '0x6a6d78db',
    transactionHash: `0x${'b'.repeat(64)}`,
    transactionIndex: '0x2',
    logIndex: '0x167',
    ...overrides,
  };
}

const filter = {
  addresses: [SPOKE],
  topic0: [SUPPLY, WITHDRAW],
  fromBlock: 100,
  toBlock: 200,
};

describe('ViemLogReader', () => {
  beforeEach(() => {
    vi.unstubAllGlobals();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('sends the topic filter to the provider', async () => {
    const calls = stubRpc(() => ok([]));

    await build().getLogs(filter);

    // viem's typed `getLogs` derives topics from an ABI and silently drops a
    // `topics` argument — the same log count comes back filtered or not. This
    // asserts the filter actually reaches the wire, because one that quietly
    // does nothing means fetching every event the contract emits.
    expect(calls).toEqual([
      {
        method: 'eth_getLogs',
        params: [
          {
            address: [SPOKE],
            fromBlock: '0x64',
            toBlock: '0xc8',
            topics: [[SUPPLY, WITHDRAW]],
          },
        ],
      },
    ]);
  });

  it('omits the topic filter entirely when no signatures are given', async () => {
    const calls = stubRpc(() => ok([]));

    await build().getLogs({ ...filter, topic0: [] });

    // `topics: [[]]` would match nothing at all rather than everything.
    expect(calls[0]?.params).toEqual([{ address: [SPOKE], fromBlock: '0x64', toBlock: '0xc8' }]);
  });

  it('converts every hex quantity to a number', async () => {
    stubRpc(() => ok([rpcLog()]));

    await expect(build().getLogs(filter)).resolves.toEqual([
      {
        address: SPOKE,
        topics: [SUPPLY],
        data: '0x00',
        blockNumber: 100,
        blockHash: `0x${'a'.repeat(64)}`,
        blockTimestamp: 0x6a6d78db,
        transactionHash: `0x${'b'.repeat(64)}`,
        transactionIndex: 2,
        logIndex: 359,
      },
    ]);
  });

  it('uses the provider blockTimestamp without a single extra request', async () => {
    const calls = stubRpc(() => ok([rpcLog(), rpcLog({ logIndex: '0x168' })]));

    const logs = await build().getLogs(filter);

    expect(logs.map((l) => l.blockTimestamp)).toEqual([0x6a6d78db, 0x6a6d78db]);
    expect(calls.filter((c) => c.method === 'eth_getBlockByNumber')).toHaveLength(0);
  });

  it('reads one header per distinct block when the provider omits the timestamp', async () => {
    const calls = stubRpc((call) => {
      if (call.method === 'eth_getBlockByNumber') {
        return ok({ number: '0x64', hash: `0x${'a'.repeat(64)}`, timestamp: '0x999' });
      }
      return ok([
        rpcLog({ blockTimestamp: undefined, logIndex: '0x1' }),
        rpcLog({ blockTimestamp: undefined, logIndex: '0x2' }),
        rpcLog({ blockTimestamp: undefined, logIndex: '0x3' }),
      ]);
    });

    const logs = await build().getLogs(filter);

    expect(logs.map((l) => l.blockTimestamp)).toEqual([0x999, 0x999, 0x999]);
    // Three logs, one block: one header read, not three.
    expect(calls.filter((c) => c.method === 'eth_getBlockByNumber')).toHaveLength(1);
  });

  // The literal strings four public endpoints returned on 2026-08-01. Two of
  // them share viem's error class and JSON-RPC code with the negative case
  // below, so nothing but the message distinguishes them.
  it.each([
    ['drpc', 35, 'ranges over 10000 blocks are not supported on free plan'],
    ['1rpc', -32602, 'eth_getLogs is limited to 0 - 50 blocks range'],
    [
      'nodies',
      -32001,
      'Block range too large: maximum allowed is 50 blocks on your current plan. Upgrade your subscription at https://nodies.app for larger ranges.',
    ],
  ])('treats the %s range rejection as narrowable', async (_provider, code, message) => {
    stubRpc(() => rpcError(code, message));

    await expect(build().getLogs(filter)).rejects.toBeInstanceOf(LogRangeTooLargeError);
    await expect(build().getLogs(filter)).rejects.toThrow(/100\.\.200/);
  });

  // Both observed on public endpoints, and both must fall through to an
  // ordinary retry. The second is the one that earns the two-part match: it
  // carries "limit" and "exceeded", so a matcher keyed on size words alone would
  // shrink the range forever against a problem that has nothing to do with size.
  it.each([
    [
      'an archive-plan refusal',
      -32602,
      'Archive requests require a personal token. Get one at: https://www.allnodes.com/publicnode',
    ],
    ['a rate limit', -32005, 'Rate limit exceeded on Nodies public endpoint'],
  ])('does not mistake %s for an over-wide range', async (_case, code, message) => {
    // Note the first shares viem's InvalidParamsRpcError and code -32602 with
    // the 1rpc range rejection above, which is why the match cannot key on
    // either the error class or the JSON-RPC code.
    stubRpc(() => rpcError(code, message));

    const failure = await build()
      .getLogs(filter)
      .catch((error: unknown) => error);

    expect(failure).not.toBeInstanceOf(LogRangeTooLargeError);
    expect(failure).toBeInstanceOf(Error);
  });

  it('rethrows an ordinary transport failure unchanged', async () => {
    stubRpc(() => new Response('upstream error', { status: 500 }));

    await expect(build().getLogs(filter)).rejects.not.toBeInstanceOf(LogRangeTooLargeError);
  });
});
