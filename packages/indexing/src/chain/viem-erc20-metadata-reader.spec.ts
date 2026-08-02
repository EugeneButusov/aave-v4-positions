import { encodeAbiParameters, encodeErrorResult, numberToHex, stringToHex, type Hex } from 'viem';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ViemErc20MetadataReader } from './viem-erc20-metadata-reader';

/** `symbol()`, `name()`, `decimals()`, derived rather than pasted. */
const SELECTOR = {
  symbol: '0x95d89b41',
  name: '0x06fdde03',
  decimals: '0x313ce567',
} as const;

const TOKEN = '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48';
const AT_BLOCK = 25_652_782n;

/** What a call returns: raw return data, or a JSON-RPC error. */
type Answer = { readonly data: Hex } | { readonly revert: Hex | null };

/** One `eth_call`, reduced to what these tests assert on. */
interface Call {
  readonly calldata: Hex;
  readonly block: string;
}

function data(value: Hex): Answer {
  return { data: value };
}

const asString = (value: string): Answer =>
  data(encodeAbiParameters([{ type: 'string' }], [value]));
const asUint8 = (value: number): Answer => data(encodeAbiParameters([{ type: 'uint8' }], [value]));

/** A `bytes32` return, which is what the MKR/SAI generation actually does. */
const asBytes32 = (value: string): Answer =>
  data(encodeAbiParameters([{ type: 'bytes32' }], [stringToHex(value, { size: 32 })]));

/** Nothing at all — an EOA, or a method the contract does not implement. */
const EMPTY: Answer = data('0x');

/** A standard `Error(string)` revert payload, which is what a node returns. */
const REVERT_REASON: Hex = encodeErrorResult({
  abi: [{ type: 'error', name: 'Error', inputs: [{ type: 'string' }] }],
  errorName: 'Error',
  args: ['no symbol here'],
});

function response(body: unknown): Response {
  return new Response(JSON.stringify({ jsonrpc: '2.0', id: 1, ...(body as object) }), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
}

/**
 * Answers `eth_call` per selector, so one token can be mute on `symbol()` and
 * fluent on `name()` — which is the whole reason the reader reads them apart.
 *
 * Anything not named answers empty, which is how a real contract missing a
 * method behaves rather than a special case for the test.
 */
function stubToken(answers: Partial<Record<keyof typeof SELECTOR, Answer>>): Call[] {
  const seen: Call[] = [];

  vi.stubGlobal(
    'fetch',
    vi.fn((_input: string | URL | Request, init?: RequestInit) => {
      const body: unknown = JSON.parse(typeof init?.body === 'string' ? init.body : '{}');
      const params = (body as { params?: [{ data?: Hex }, string] }).params;
      const calldata = params?.[0]?.data ?? '0x';
      seen.push({ calldata, block: params?.[1] ?? '' });

      const entry = Object.entries(SELECTOR).find(([, selector]) => calldata.startsWith(selector));
      const answer = entry ? answers[entry[0] as keyof typeof SELECTOR] : undefined;
      const resolved = answer ?? EMPTY;

      if (!('revert' in resolved)) return Promise.resolve(response({ result: resolved.data }));

      // A node may omit `data` on a revert. It changes the chain below the
      // classified link but not the link itself, and both shapes occur in the
      // wild, so both are exercised.
      return Promise.resolve(
        response({
          error: {
            code: 3,
            message: 'execution reverted',
            ...(resolved.revert !== null && { data: resolved.revert }),
          },
        }),
      );
    }),
  );

  return seen;
}

function build(): ViemErc20MetadataReader {
  return new ViemErc20MetadataReader({
    rpcUrls: ['https://provider.invalid'],
    rpcTimeoutMs: 1_000,
  });
}

describe('ViemErc20MetadataReader', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('reads an ordinary token', async () => {
    stubToken({ symbol: asString('USDC'), name: asString('USD Coin'), decimals: asUint8(6) });

    expect(await build().read(TOKEN, AT_BLOCK)).toEqual({
      symbol: 'USDC',
      name: 'USD Coin',
      decimals: 6,
      failures: [],
    });
  });

  it('pins every call to the block it was given', async () => {
    const seen = stubToken({ symbol: asString('USDC'), name: asString('X'), decimals: asUint8(6) });
    await build().read(TOKEN, AT_BLOCK);

    // Three calls at `latest` across a failover list are three calls at
    // possibly three heights. Metadata is immutable in practice, so this is
    // about the reads agreeing with each other.
    expect(seen).toHaveLength(3);
    expect(seen.every((call) => call.block === numberToHex(AT_BLOCK))).toBe(true);
  });

  describe('the bytes32 generation', () => {
    it('falls back and trims the padding', async () => {
      stubToken({ symbol: asBytes32('MKR'), name: asBytes32('Maker'), decimals: asUint8(18) });

      // The raw decode is "MKR" followed by twenty-nine NULs. Two things
      // remove them and only one is load-bearing: the mutation test shows
      // dropping `{ size: 32 }` changes nothing, because the control strip
      // catches NULs anyway. See the left-padded case below for where the
      // strip is the only thing that works.
      expect(await build().read(TOKEN, AT_BLOCK)).toMatchObject({
        symbol: 'MKR',
        name: 'Maker',
        failures: [],
      });
    });

    it('does not key the fallback on the error the decoder happens to raise', async () => {
      // Left-padded rather than right-padded, which some tokens genuinely get
      // wrong. It matters because the string decode then reads the first word
      // as a *plausible* offset (0x4d4b52 = 5,065,042) and seeks past the end
      // of a 32-byte buffer — raising PositionOutOfBoundsError, not
      // IntegerOutOfRangeError. Keying the retry on the latter would return
      // null here for a token whose symbol is perfectly readable.
      stubToken({ symbol: data(`0x${'00'.repeat(29)}4d4b52`) });

      expect((await build().read(TOKEN, AT_BLOCK)).symbol).toBe('MKR');
    });
  });

  describe('a token that will not answer', () => {
    it('reports a revert per field rather than throwing', async () => {
      stubToken({
        symbol: { revert: REVERT_REASON },
        name: asString('Quiet Token'),
        decimals: asUint8(8),
      });

      const metadata = await build().read(TOKEN, AT_BLOCK);

      // The fields fail independently. Reading them as a unit would throw away
      // the half that worked.
      expect(metadata.symbol).toBeNull();
      expect(metadata.name).toBe('Quiet Token');
      expect(metadata.decimals).toBe(8);
      expect(metadata.failures).toEqual(['symbol: ContractFunctionRevertedError']);
    });

    it('classifies a bare revert the same as one carrying a reason', async () => {
      stubToken({ symbol: { revert: null }, name: asString('X'), decimals: asUint8(8) });

      // A node may or may not return `data` for a revert. It changes the cause
      // chain below the classified link, which is exactly why the reason is
      // read from that link and not from the bottom of the chain — there it
      // would be `RpcRequestError`, the same thing a timeout says.
      expect((await build().read(TOKEN, AT_BLOCK)).failures).toEqual([
        'symbol: ContractFunctionRevertedError',
      ]);
    });

    it('reports an empty return, which is also what an EOA gives', async () => {
      stubToken({});

      const metadata = await build().read(TOKEN, AT_BLOCK);

      expect(metadata).toMatchObject({ symbol: null, name: null, decimals: null });
      // In field order, not completion order — three concurrent reads settle in
      // whatever order the provider answers.
      expect(metadata.failures).toEqual([
        'symbol: ContractFunctionZeroDataError',
        'name: ContractFunctionZeroDataError',
        'decimals: ContractFunctionZeroDataError',
      ]);
    });
  });

  describe('a token that answers with something hostile', () => {
    it('strips control characters from the string path', async () => {
      const withNewline = `USD${String.fromCharCode(10)}C`;
      stubToken({ symbol: asString(withNewline) });

      // The ABI string path runs `bytesToString` with no validation, so this
      // arrives intact and would split a log line downstream.
      expect((await build().read(TOKEN, AT_BLOCK)).symbol).toBe('USDC');
    });

    it('strips control characters from the bytes32 path too', async () => {
      stubToken({ symbol: asBytes32(`MK${String.fromCharCode(7)}R`) });

      expect((await build().read(TOKEN, AT_BLOCK)).symbol).toBe('MKR');
    });

    it('truncates a label that is far too long', async () => {
      stubToken({ name: asString('A'.repeat(5_000)) });

      expect((await build().read(TOKEN, AT_BLOCK)).name).toHaveLength(128);
    });

    it('never ends a truncated label on half a surrogate pair', async () => {
      // 127 filler plus an astral character, so the cap lands mid-pair.
      stubToken({ name: asString(`${'A'.repeat(127)}😀😀`) });

      const { name } = await build().read(TOKEN, AT_BLOCK);

      // A lone surrogate is not representable in UTF-8, so it is not something
      // to hand a database driver. Dropping it costs one character.
      expect(name).toHaveLength(127);
      expect(name?.endsWith('A')).toBe(true);
    });

    it('reports an all-whitespace label as absent', async () => {
      stubToken({ symbol: asString('   ') });

      // A symbol of three spaces would make "has a label" true for something
      // that renders as absence.
      expect((await build().read(TOKEN, AT_BLOCK)).symbol).toBeNull();
    });

    it('keeps bytes that are not UTF-8, rather than inventing a label', async () => {
      stubToken({ symbol: data(`0x${'ff'.repeat(32)}`) });

      // `hexToString` yields U+FFFD and never throws. A visibly broken symbol
      // is more honest than a null that claims the token has none.
      expect((await build().read(TOKEN, AT_BLOCK)).symbol).toMatch(/�/);
    });

    it('refuses a decimals that cannot be a uint8', async () => {
      stubToken({
        symbol: asString('ODD'),
        name: asString('Odd Token'),
        decimals: data(encodeAbiParameters([{ type: 'uint256' }], [999n])),
      });

      // 999, not 2^200. viem decodes the whole 32-byte word for a `uint8`
      // output and does not mask it — but it *does* throw above 2^248, so an
      // absurd value is caught by the decoder and never reaches the range
      // check. 999 is the interesting size: it decodes cleanly, and unchecked
      // it would surface later as an amount off by an impossible power of ten.
      const metadata = await build().read(TOKEN, AT_BLOCK);
      expect(metadata.decimals).toBeNull();
      expect(metadata.failures).toEqual(['decimals: out of range (999)']);
    });
  });
});
