import {
  decodeAbiParameters,
  encodeAbiParameters,
  encodeErrorResult,
  toFunctionSelector,
  type Hex,
} from 'viem';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ViemReservePriceReader } from './viem-reserve-price-reader';

/**
 * Derived rather than pasted, for the reason the event topics are: a wrong
 * hardcoded selector matches nothing, which reads exactly like a contract that
 * does not implement the method.
 */
const SELECTOR = {
  batch: toFunctionSelector('getReservesPrices(uint256[])'),
  single: toFunctionSelector('getReservePrice(uint256)'),
} as const;

const ORACLE = '0x99b2b6cea9c3d2fd8f4d90f86741c44b212a6127';
const AT_BLOCK = 25_652_782n;

/** Three reserves, which is enough for "one of them is broken" to mean something. */
const RESERVES = ['0', '1', '13'];

type Answer = { readonly data: Hex } | { readonly revert: Hex | null };

/** One `eth_call`, reduced to what these tests assert on. */
interface Call {
  readonly selector: Hex;
  readonly block: string;
  /** The reserve a single-price call asked about; null for the batch. */
  readonly reserveId: string | null;
}

const asPrices = (values: readonly bigint[]): Answer => ({
  data: encodeAbiParameters([{ type: 'uint256[]' }], [[...values]]),
});

const asPrice = (value: bigint): Answer => ({
  data: encodeAbiParameters([{ type: 'uint256' }], [value]),
});

/** What `InvalidPrice(uint256)` looks like on the wire. */
const invalidPrice = (reserveId: bigint): Answer => ({
  revert: encodeErrorResult({
    abi: [{ type: 'error', name: 'InvalidPrice', inputs: [{ type: 'uint256' }] }],
    errorName: 'InvalidPrice',
    args: [reserveId],
  }),
});

function response(body: unknown): Response {
  return new Response(JSON.stringify({ jsonrpc: '2.0', id: 1, ...(body as object) }), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
}

function reserveOf(calldata: Hex): string | null {
  if (!calldata.startsWith(SELECTOR.single)) return null;
  const [value] = decodeAbiParameters([{ type: 'uint256' }], `0x${calldata.slice(10)}`);
  return value.toString();
}

/**
 * Answers the batch once and each single call by reserve, so "the batch reverts
 * but twelve of fourteen reserves are fine" is expressible — which is the whole
 * situation the fallback exists for.
 */
function stubOracle(answers: {
  batch?: Answer;
  single?: Record<string, Answer>;
  /** Rejects every request, standing in for a provider that is not answering. */
  offline?: boolean;
}): Call[] {
  const seen: Call[] = [];

  vi.stubGlobal(
    'fetch',
    vi.fn((_input: string | URL | Request, init?: RequestInit) => {
      const body: unknown = JSON.parse(typeof init?.body === 'string' ? init.body : '{}');
      const params = (body as { params?: [{ data?: Hex }, string] }).params;
      const calldata = params?.[0]?.data ?? '0x';

      seen.push({
        selector: calldata.slice(0, 10) as Hex,
        block: params?.[1] ?? '',
        reserveId: reserveOf(calldata),
      });

      if (answers.offline === true) return Promise.reject(new Error('socket hang up'));

      const reserveId = reserveOf(calldata);
      const answer =
        reserveId === null ? answers.batch : (answers.single?.[reserveId] ?? asPrice(1n));

      if (answer === undefined) return Promise.resolve(response({ result: '0x' }));
      if (!('revert' in answer)) return Promise.resolve(response({ result: answer.data }));

      return Promise.resolve(
        response({
          error: {
            code: 3,
            message: 'execution reverted',
            ...(answer.revert !== null && { data: answer.revert }),
          },
        }),
      );
    }),
  );

  return seen;
}

function reader(): ViemReservePriceReader {
  return new ViemReservePriceReader({ rpcUrls: ['http://oracle.invalid'], rpcTimeoutMs: 2_000 });
}

describe('ViemReservePriceReader', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  describe('the batch', () => {
    it('reads every reserve in one call', async () => {
      const calls = stubOracle({ batch: asPrices([187_522_000_000n, 99_971_505n, 1n]) });

      const result = await reader().read(ORACLE, RESERVES, AT_BLOCK);

      expect([...result.prices]).toEqual([
        ['0', '187522000000'],
        ['1', '99971505'],
        ['13', '1'],
      ]);
      expect(result.failures).toEqual([]);
      // One call, which is the point of the batch. Fourteen round trips would
      // work and would be fourteen chances to land on different heights.
      expect(calls).toHaveLength(1);
      expect(calls[0]?.selector).toBe(SELECTOR.batch);
    });

    it('pins every call to the block it was given', async () => {
      const calls = stubOracle({ batch: asPrices([1n, 2n, 3n]) });

      const result = await reader().read(ORACLE, RESERVES, AT_BLOCK);

      expect(result.blockNumber).toBe(AT_BLOCK);
      // §7.1 weighs collateral against debt, so two prices from two heights do
      // not merely disagree — they misprice the ratio the health factor is.
      expect(calls.map((call) => call.block)).toEqual([`0x${AT_BLOCK.toString(16)}`]);
    });

    it('asks for nothing when there is nothing listed', async () => {
      const calls = stubOracle({ batch: asPrices([]) });

      const result = await reader().read(ORACLE, [], AT_BLOCK);

      expect(result.prices.size).toBe(0);
      expect(result.failures).toEqual([]);
      // A cold start has no reserves folded yet. Calling with an empty array
      // would be a round trip to learn what the caller already knew.
      expect(calls).toEqual([]);
    });

    it('refuses a non-positive price rather than storing it', async () => {
      // Should be impossible — the oracle reverts rather than answer zero
      // (§7.4). This is about what happens if that stops being true: the
      // column's own CHECK would reject the insert and take the whole batch of
      // good prices with it.
      stubOracle({ batch: asPrices([187_522_000_000n, 0n, 1n]) });

      const result = await reader().read(ORACLE, RESERVES, AT_BLOCK);

      expect([...result.prices.keys()]).toEqual(['0', '13']);
      expect(result.failures).toEqual(['reserve 1: non-positive price (0)']);
    });
  });

  describe('when the batch reverts', () => {
    it('isolates the one reserve the oracle refused', async () => {
      const calls = stubOracle({
        batch: invalidPrice(1n),
        single: { '0': asPrice(187_522_000_000n), '1': invalidPrice(1n), '13': asPrice(1n) },
      });

      const result = await reader().read(ORACLE, RESERVES, AT_BLOCK);

      // The whole reason the fallback exists: one broken feed costs one
      // reserve, not the other thirteen.
      expect([...result.prices]).toEqual([
        ['0', '187522000000'],
        ['13', '1'],
      ]);
      expect(result.failures).toHaveLength(2);
      expect(result.failures[1]).toBe('reserve 1: ContractFunctionRevertedError');
      expect(calls).toHaveLength(1 + RESERVES.length);
    });

    it('falls back when the answer is the wrong length', async () => {
      // Positional pairing is the one failure here that produces plausible
      // numbers rather than an error, so a short answer must not be zipped.
      const calls = stubOracle({
        batch: asPrices([187_522_000_000n, 99_971_505n]),
        single: { '0': asPrice(1n), '1': asPrice(2n), '13': asPrice(3n) },
      });

      const result = await reader().read(ORACLE, RESERVES, AT_BLOCK);

      expect([...result.prices]).toEqual([
        ['0', '1'],
        ['1', '2'],
        ['13', '3'],
      ]);
      expect(result.failures[0]).toBe('getReservesPrices: returned 2 prices for 3 reserves');
      expect(calls).toHaveLength(1 + RESERVES.length);
    });
  });

  describe('when the oracle never answers', () => {
    it('does not ask again one at a time', async () => {
      const calls = stubOracle({ offline: true });

      const result = await reader().read(ORACLE, RESERVES, AT_BLOCK);

      expect(result.prices.size).toBe(0);
      // The gate that keeps a dead provider from being asked four times per
      // refresh instead of once. A revert means the oracle answered and
      // refused, so asking one at a time isolates which reserve; a dead socket
      // means it never answered, and three more calls learn nothing.
      expect(calls).toHaveLength(1);
      expect(result.failures).toHaveLength(1);
      expect(result.failures[0]).toMatch(/^getReservesPrices: /);
    });
  });
});
