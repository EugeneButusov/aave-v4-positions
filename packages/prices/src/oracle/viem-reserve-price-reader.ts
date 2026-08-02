import { IAaveOracleV4_ABI } from '@aave-dao/aave-address-book/abis';
import { Inject, Injectable } from '@nestjs/common';
import {
  CHAIN_CLIENT_OPTIONS,
  ViemChainClient,
  type Address,
  type ChainClientOptions,
} from '@packages/indexing';

import type { ReservePriceReader, ReservePrices } from './reserve-price-reader';

/**
 * Names what went wrong, from the classified link of viem's cause chain.
 *
 * The same second-link rule `ViemErc20MetadataReader` measured against 2.55.10:
 * viem wraps every contract read in `ContractFunctionExecutionError`, so the
 * outer name is identical for a revert and a timeout, and the innermost is the
 * transport's `RpcRequestError` for both. Duplicated rather than shared because
 * the packages are independent and three lines is less coupling than a new
 * export on a port package's surface.
 */
function describeFailure(error: unknown): string {
  if (!(error instanceof Error)) return 'unknown error';
  return error.cause instanceof Error ? error.cause.name : error.name;
}

/** The oracle ran and rejected the call, rather than never answering. */
const REVERTED = 'ContractFunctionRevertedError';

/**
 * Adapts viem to the {@link ReservePriceReader} port.
 *
 * **One call for the whole Spoke, then one per reserve only if that fails.**
 * `getReservesPrices` takes every `reserveId` at once, which is both cheaper
 * than fourteen round trips and the only way all fourteen prices come from one
 * evaluation. But it is a batch over a contract that reverts rather than
 * answers — `InvalidPrice(uint256 reserveId)`, and §7.4 records that the oracle
 * reverts on zero — so a single broken feed takes the other thirteen with it.
 * The fallback is what keeps one bad reserve costing one bad reserve.
 *
 * **The fallback is gated on the failure being a revert**, and that gate is the
 * point rather than an optimisation. A revert means the oracle answered and
 * refused, so asking one at a time isolates which reserve it refused. A timeout
 * or a dead provider means it never answered, and asking fourteen more times
 * would produce fourteen more timeouts against an endpoint already in trouble —
 * every `RESERVE_PRICE_RETRY_MS`, forever.
 *
 * **Nothing throws out of here.** A price that cannot be read is an absent entry
 * and a recorded reason, because the caller's correct response is to leave the
 * last good price in place rather than to stop.
 */
@Injectable()
export class ViemReservePriceReader extends ViemChainClient implements ReservePriceReader {
  // Declared rather than inherited so the injection metadata sits on this
  // class; Nest does not walk up to a base constructor's parameter decorators.
  constructor(@Inject(CHAIN_CLIENT_OPTIONS) options: ChainClientOptions) {
    super(options);
  }

  async read(
    oracle: Address,
    reserveIds: readonly string[],
    atBlock: bigint,
  ): Promise<ReservePrices> {
    if (reserveIds.length === 0) {
      return { blockNumber: atBlock, prices: new Map(), failures: [] };
    }

    try {
      const answers = await this.client.readContract({
        address: this.hex(oracle),
        abi: IAaveOracleV4_ABI,
        functionName: 'getReservesPrices',
        args: [reserveIds.map((id) => BigInt(id))],
        blockNumber: atBlock,
      });

      // Positional, so a short answer would silently pair prices with the wrong
      // reserves — which is the one failure here that produces plausible
      // numbers rather than an error. Checked rather than trusted, and a
      // mismatch drops to the one-at-a-time path where the pairing is explicit.
      if (answers.length === reserveIds.length) return this.zip(reserveIds, answers, atBlock);

      return this.oneByOne(
        oracle,
        reserveIds,
        atBlock,
        `getReservesPrices: returned ${String(answers.length)} prices for ${String(reserveIds.length)} reserves`,
      );
    } catch (batchError) {
      const failure = describeFailure(batchError);

      // Never answered. One at a time would ask the same dead endpoint fourteen
      // more times and learn nothing.
      if (failure !== REVERTED) {
        return {
          blockNumber: atBlock,
          prices: new Map(),
          failures: [`getReservesPrices: ${failure}`],
        };
      }

      return this.oneByOne(oracle, reserveIds, atBlock, `getReservesPrices: ${failure}`);
    }
  }

  /** Pairs a positional answer with the ids it was asked about. */
  private zip(
    reserveIds: readonly string[],
    answers: readonly bigint[],
    atBlock: bigint,
  ): ReservePrices {
    const prices = new Map<string, string>();
    const failures: string[] = [];

    for (const [index, reserveId] of reserveIds.entries()) {
      const price = answers[index];

      // Zero should be impossible — the oracle reverts rather than return one
      // (§7.4) — so this is about what to do if that ever stops being true.
      // Refusing it keeps the invention out of the store, where the column's
      // own CHECK would otherwise reject the insert and take the whole batch of
      // good prices down with it.
      if (price === undefined || price <= 0n) {
        failures.push(`reserve ${reserveId}: non-positive price (${String(price)})`);
        continue;
      }

      prices.set(reserveId, price.toString());
    }

    return { blockNumber: atBlock, prices, failures };
  }

  /** One `getReservePrice` per reserve, so one refusal costs one reserve. */
  private async oneByOne(
    oracle: Address,
    reserveIds: readonly string[],
    atBlock: bigint,
    batchFailure: string,
  ): Promise<ReservePrices> {
    const prices = new Map<string, string>();
    const failures: string[] = [batchFailure];

    for (const reserveId of reserveIds) {
      try {
        // Sequential, for the reason `reconcile-hub` gives: a public endpoint
        // rate-limits a burst of these long before fourteen calls become slow,
        // and this path only runs when the batch has already failed.
        // oxlint-disable-next-line no-await-in-loop
        const price = await this.client.readContract({
          address: this.hex(oracle),
          abi: IAaveOracleV4_ABI,
          functionName: 'getReservePrice',
          args: [BigInt(reserveId)],
          blockNumber: atBlock,
        });

        if (price <= 0n) {
          failures.push(`reserve ${reserveId}: non-positive price (${String(price)})`);
          continue;
        }

        prices.set(reserveId, price.toString());
      } catch (error) {
        failures.push(`reserve ${reserveId}: ${describeFailure(error)}`);
      }
    }

    return { blockNumber: atBlock, prices, failures };
  }

  /**
   * The adapter boundary's one cast, for the reason `ViemLogReader` gives:
   * addresses are lower-cased on the way in and `toLowerCase()` returns
   * `string`, where viem types them as a `0x${string}` template.
   */
  private hex(address: Address): `0x${string}` {
    // oxlint-disable-next-line no-unsafe-type-assertion
    return address as `0x${string}`;
  }
}
