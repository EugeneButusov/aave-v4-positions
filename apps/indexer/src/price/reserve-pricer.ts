import { Inject, Injectable, Logger } from '@nestjs/common';
import {
  RESERVE_LISTINGS,
  RESERVE_PRICE_READER,
  RESERVE_PRICE_STORE,
  type ReserveListings,
  type ReservePriceReader,
  type ReservePriceStore,
} from '@aave-positions/positions';
import { CHAIN_CLIENT, type Address, type ChainClient } from '@packages/indexing';

import type { PriceRequest } from './price-args';

export interface PriceResult {
  readonly asked: number;
  readonly priced: number;
  /** Reserves the oracle would not price, so their gap is still open. */
  readonly unpriced: readonly string[];
  readonly failures: readonly string[];
}

/**
 * One pass of pricing, run by hand.
 *
 * Shares the ports the processor uses rather than the processor itself: the
 * scheduling — the latch, the two delays, the swallow-and-carry-on — is exactly
 * what a command should not inherit. Here a failure is the operator's to see,
 * and the exit code says so.
 */
@Injectable()
export class ReservePricer {
  private readonly logger = new Logger(ReservePricer.name);

  constructor(
    @Inject(RESERVE_LISTINGS) private readonly listings: ReserveListings,
    @Inject(RESERVE_PRICE_STORE) private readonly store: ReservePriceStore,
    @Inject(RESERVE_PRICE_READER) private readonly reader: ReservePriceReader,
    @Inject(CHAIN_CLIENT) private readonly chain: ChainClient,
  ) {}

  async run(
    chainId: number,
    spoke: Address,
    oracle: Address,
    request: PriceRequest,
  ): Promise<PriceResult> {
    const reserveIds = await this.listings.forSpoke(chainId, spoke);
    if (reserveIds.length === 0) {
      // Said plainly rather than reported as success. A cold start has nothing
      // to price, and "priced 0 reserves" reads like the oracle answered.
      this.logger.warn(`no reserves registered for spoke ${spoke} on chain ${chainId}`, 'Price');
      return { asked: 0, priced: 0, unpriced: [], failures: [] };
    }

    // The head, and one block for the whole batch — the same pinning the
    // processor does, for the same reason: §7.1 weighs collateral against debt,
    // so prices from two heights misprice the ratio rather than merely
    // disagreeing.
    const head = BigInt(await this.chain.getHeadBlockNumber());
    const { prices, failures } = await this.reader.read(oracle, reserveIds, head);

    for (const failure of failures) this.logger.warn(failure, 'Price');

    for (const reserveId of reserveIds) {
      const price = prices.get(reserveId);
      this.logger.log(`reserve ${reserveId}  ${price ?? '—'}`, 'Price');
    }

    if (!request.dryRun) {
      await this.store.put(
        [...prices].map(([reserveId, price]) => ({ chainId, spoke, reserveId, price })),
      );
    }

    return {
      asked: reserveIds.length,
      priced: prices.size,
      unpriced: reserveIds.filter((reserveId) => !prices.has(reserveId)),
      failures,
    };
  }
}
