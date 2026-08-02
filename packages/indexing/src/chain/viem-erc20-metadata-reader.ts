import { Inject, Injectable } from '@nestjs/common';
import { erc20Abi, erc20Abi_bytes32, hexToString, type Hex } from 'viem';

import { CHAIN_CLIENT_OPTIONS, type Address, type ChainClientOptions } from './chain-client';
import type { Erc20MetadataReader, TokenMetadata } from './erc20-metadata-reader';
import { ViemChainClient } from './viem-chain-client';

/**
 * How long a label may be before it is truncated.
 *
 * Not a protocol limit — there is none, and a token can return a megabyte. The
 * cap is about what a caller can be handed, not about what is valid: 128 is far
 * above every real symbol and name, and far below anything usable as a payload.
 */
const MAX_LABEL_LENGTH = 128;

/** End of the C0 block, then DEL and the end of C1. */
const C0_END = 0x1f;
const DEL = 0x7f;
const C1_END = 0x9f;

/** The high half of a surrogate pair, which must not be left dangling. */
const HIGH_SURROGATE_START = 0xd800;
const HIGH_SURROGATE_END = 0xdbff;

/**
 * Removes C0 and C1 control characters.
 *
 * **Stripped rather than escaped**, and from *both* decode paths, because
 * neither trims. `hexToString` yields U+FFFD for bytes that are not UTF-8 and
 * never throws; the ABI `string` path runs `bytesToString` with no size and no
 * validation at all. A newline inside a symbol has no meaning to recover, and
 * the damage it does — a split log line, a broken table — happens downstream of
 * here, where nothing looks for it.
 *
 * Scanned by UTF-16 unit rather than by code point, and that is safe rather
 * than sloppy: every control character is below U+00A0, so none can be half of
 * a surrogate pair and no pair can be mistaken for one. Written as bounds
 * rather than as a character class so no control character has to appear in
 * this file to describe one.
 */
function stripControls(value: string): string {
  let kept = '';

  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code > C0_END && (code < DEL || code > C1_END)) kept += value[index];
  }

  return kept;
}

/**
 * Truncates to {@link MAX_LABEL_LENGTH} without ending on half a character.
 *
 * `slice` counts UTF-16 units, so a cut can land between the two halves of a
 * surrogate pair and leave a lone one — not representable in UTF-8, and so not
 * something to hand a database driver. Dropping the orphan costs one character
 * off an already-truncated label.
 *
 * Grapheme clusters are deliberately *not* preserved: a ZWJ sequence cut in the
 * middle renders as two glyphs instead of one, which is cosmetic, and
 * `Intl.Segmenter` is a lot of machinery to buy that on a token symbol.
 */
function truncate(value: string): string {
  if (value.length <= MAX_LABEL_LENGTH) return value;

  const cut = value.slice(0, MAX_LABEL_LENGTH);
  const last = cut.charCodeAt(cut.length - 1);
  const dangling = last >= HIGH_SURROGATE_START && last <= HIGH_SURROGATE_END;

  return dangling ? cut.slice(0, -1) : cut;
}

/**
 * Trims a label to something safe to hand onward, or null if nothing is left.
 *
 * All-whitespace collapses to null on purpose: a symbol of three spaces is not
 * a symbol, and keeping it would make "has a label" true for something that
 * renders as absence.
 */
function sanitise(value: string): string | null {
  const cleaned = stripControls(value).trim();
  return cleaned.length === 0 ? null : truncate(cleaned);
}

/** `uint8` on chain, but nothing enforces that what comes back fits one. */
const MAX_DECIMALS = 255;

/** One field's outcome: the value, or why there isn't one. Never both. */
interface Attempt<T> {
  readonly value: T | null;
  readonly failure: string | null;
}

/**
 * Names what went wrong, from the classified link of viem's cause chain.
 *
 * viem wraps every contract read in `ContractFunctionExecutionError`, so the
 * outer name is identical for a revert, an empty return and a decode failure —
 * and the *innermost* is no better, because it is the transport's. The
 * classification sits at the second link. Measured against 2.55.10:
 *
 * ```
 * revert   ContractFunctionExecutionError > ContractFunctionRevertedError > … > RpcRequestError
 * 0x       ContractFunctionExecutionError > ContractFunctionZeroDataError > AbiDecodingZeroDataError
 * bytes32  ContractFunctionExecutionError > IntegerOutOfRangeError
 * ```
 *
 * Walking to the bottom would report `RpcRequestError` for a revert — true, and
 * useless, since it is what a timeout says too.
 */
function describeFailure(error: unknown): string {
  if (!(error instanceof Error)) return 'unknown error';
  return error.cause instanceof Error ? error.cause.name : error.name;
}

/**
 * Adapts viem to the {@link Erc20MetadataReader} port.
 *
 * **Nothing throws out of here.** A token is a third-party contract that may
 * revert, return nothing, return the wrong type, or not exist at all, and none
 * of those is this process's problem to fail on. Each becomes a null field and
 * a recorded reason.
 *
 * **The `bytes32` fallback keys on any throw, not on a particular error.** The
 * MKR/SAI generation declares `symbol()` as `bytes32`, and decoding that as a
 * `string` reads the first word as a byte offset — surfacing as
 * `IntegerOutOfRangeError`, a name that says nothing about the real problem.
 *
 * viem wraps everything in `ContractFunctionExecutionError` and classifies at
 * the next link down — measured against 2.55.10, not predicted:
 *
 * - a revert, with or without reason data → `ContractFunctionRevertedError`
 * - `0x`, or no code at all               → `ContractFunctionZeroDataError`
 * - a `bytes32` return                    → `IntegerOutOfRangeError`
 * - a short or lying payload              → `PositionOutOfBoundsError`
 *
 * Four names, none of which means "wrong return type", and the set is not
 * closed — which is the whole argument for retrying on any throw. Keying the
 * fallback on `IntegerOutOfRangeError` looks tidier and silently drops the case
 * where a `bytes32` token fails some other way first.
 */
@Injectable()
export class ViemErc20MetadataReader extends ViemChainClient implements Erc20MetadataReader {
  // Declared rather than inherited so the injection metadata sits on this
  // class; Nest does not walk up to a base constructor's parameter decorators.
  constructor(@Inject(CHAIN_CLIENT_OPTIONS) options: ChainClientOptions) {
    super(options);
  }

  async read(token: Address, atBlock: bigint): Promise<TokenMetadata> {
    // All three at once, and they fail independently. A token with a `symbol()`
    // and no `name()` is common enough that reading them as a unit would throw
    // away the half that worked.
    const [symbol, name, decimals] = await Promise.all([
      this.label(token, atBlock, 'symbol'),
      this.label(token, atBlock, 'name'),
      this.decimals(token, atBlock),
    ]);

    return {
      symbol: symbol.value,
      name: name.value,
      decimals: decimals.value,
      // Assembled in field order rather than pushed as each read settles.
      // Concurrent reads finish in whatever order the provider answers, and a
      // diagnostic whose order changes between identical runs is one nobody can
      // assert on — including the specs below.
      failures: [symbol.failure, name.failure, decimals.failure].filter(
        (failure): failure is string => failure !== null,
      ),
    };
  }

  /** `symbol()` or `name()`: as a string, then as `bytes32`, then not at all. */
  private async label(
    token: Address,
    atBlock: bigint,
    functionName: 'symbol' | 'name',
  ): Promise<Attempt<string>> {
    const address = this.hex(token);

    try {
      const value = await this.client.readContract({
        address,
        abi: erc20Abi,
        functionName,
        blockNumber: atBlock,
      });
      return { value: sanitise(value), failure: null };
    } catch (stringError) {
      try {
        const raw: Hex = await this.client.readContract({
          address,
          abi: erc20Abi_bytes32,
          functionName,
          blockNumber: atBlock,
        });

        // `{ size: 32 }` is explicitness rather than necessity, and the
        // mutation test says so: removing it changes no result, because the
        // padding it trims is NUL bytes and `stripControls` removes those
        // anyway. It stays because it spells out that this is a fixed 32-byte
        // field rather than a string that happens to end in zeros — and
        // because the two guards answer different questions, one about
        // encoding and one about hostile content.
        //
        // The strip is the one carrying weight here. A *left*-padded bytes32,
        // which some tokens emit, puts the NULs in front where no trim would
        // reach them.
        return { value: sanitise(hexToString(raw, { size: 32 })), failure: null };
      } catch {
        // The string error, not the bytes32 one. A token that is simply absent
        // fails both ways, and the first attempt is the one whose reason
        // describes the token rather than describing our retry.
        return { value: null, failure: `${functionName}: ${describeFailure(stringError)}` };
      }
    }
  }

  private async decimals(token: Address, atBlock: bigint): Promise<Attempt<number>> {
    try {
      const value = await this.client.readContract({
        address: this.hex(token),
        abi: erc20Abi,
        functionName: 'decimals',
        blockNumber: atBlock,
      });

      // Range-checked here rather than trusted to the ABI. viem decodes the
      // whole 32-byte word for a `uint8` output, so a token returning a
      // `uint256` yields whatever it holds — and a decimals of 2^200 would sail
      // through as a number, to be noticed later as an amount off by an
      // impossible power of ten.
      if (!Number.isInteger(value) || value < 0 || value > MAX_DECIMALS) {
        return { value: null, failure: `decimals: out of range (${String(value)})` };
      }

      return { value, failure: null };
    } catch (error) {
      return { value: null, failure: `decimals: ${describeFailure(error)}` };
    }
  }

  /**
   * The adapter boundary's one cast, for the reason `ViemLogReader` gives:
   * addresses are lower-cased on the way in and `toLowerCase()` returns
   * `string`, where viem types them as a `0x${string}` template.
   */
  private hex(token: Address): `0x${string}` {
    // oxlint-disable-next-line no-unsafe-type-assertion
    return token as `0x${string}`;
  }
}
