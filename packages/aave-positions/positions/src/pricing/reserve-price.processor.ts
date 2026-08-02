import { Inject, Injectable, Logger } from '@nestjs/common';
import {
  CHAIN_CLIENT,
  ok,
  type Address,
  type BlockProcessor,
  type ChainClient,
  type ProcessorOutcome,
} from '@packages/indexing';

import type { ReservePriceRow } from '../store/reserve-price';
import { RESERVE_PRICE_STORE, type ReservePriceStore } from '../store/reserve-price-store';
import { RESERVE_LISTINGS, type ReserveListings } from '../store/reserve-listing-source';
import { RESERVE_PRICE_READER, type ReservePriceReader } from './reserve-price-reader';

export interface ReservePriceOptions {
  readonly chainId: number;
  /** Which Spoke's reserves are priced. A second Spoke is a second registration. */
  readonly spoke: Address;
  /** That Spoke's oracle. Per-Spoke, and keyed by `reserveId` rather than token. */
  readonly oracle: Address;
  /** How long a successful read stays good before the next one. */
  readonly refreshMs: number;
  /** How long to wait after a read that left a price stale. */
  readonly retryMs: number;
}

export const RESERVE_PRICE_OPTIONS = Symbol('RESERVE_PRICE_OPTIONS');

/**
 * Keeps what Aave prices each reserve at up to date, automatically.
 *
 * **`onBlockRange` awaits nothing**, for the reason
 * {@link TokenEnrichmentProcessor} gives at length: `dispatchToProcessors` runs
 * processors one after another, so awaiting a third-party contract read here
 * would hold the Spoke and Hub ledgers behind an oracle. Staying in step with
 * the chain is the job; a price a minute old is a price.
 *
 * **The latch is a clock, and that is the one structural difference from
 * enrichment.** Metadata is gap-driven and *terminates* — a token read once is
 * never read again, so "run when there is a gap" is a complete rule and a timer
 * would be a schedule nobody tuned. A price is never done. There is no gap to
 * close, no event that announces a new value (`AnswerUpdated` fires on the
 * Chainlink aggregator, which this deployment does not index), and the previous
 * answer is stale the moment it is written. So the question this processor asks
 * is "how long since the last read" rather than "what is missing", and the two
 * delays say what to do with each answer:
 *
 * - **everything resolved** — wait `refreshMs`. The data is as good as it gets.
 * - **anything did not** — wait `retryMs`, which is shorter. A price left
 *   standing while its neighbours moved is the thing worth fixing soonest, and
 *   §7.1 weighs them against each other.
 *
 * That is safe to skip, interrupt or lose because it is idempotent and derives
 * its work from the registry rather than from the range: a run that never
 * happens costs one refresh interval of staleness, which `pricing.stale` on the
 * response already reports.
 *
 * **Nothing is ever blanked.** A reserve the oracle refused is left out of the
 * write, so its last good price and timestamp stay put and simply age. Writing a
 * null would turn a provider hiccup into a missing USD value on a live endpoint.
 */
@Injectable()
export class ReservePriceProcessor implements BlockProcessor {
  readonly name = 'reserve-prices';

  private readonly logger = new Logger(ReservePriceProcessor.name);

  /** The run in flight, if any. Its only job is to stop a second one starting. */
  private running: Promise<void> | null = null;

  /** Epoch ms before which not to read again. Zero, so the first dispatch runs. */
  private nextRunAt = 0;

  constructor(
    @Inject(RESERVE_PRICE_OPTIONS) private readonly options: ReservePriceOptions,
    @Inject(RESERVE_LISTINGS) private readonly listings: ReserveListings,
    @Inject(RESERVE_PRICE_STORE) private readonly store: ReservePriceStore,
    @Inject(RESERVE_PRICE_READER) private readonly reader: ReservePriceReader,
    @Inject(CHAIN_CLIENT) private readonly chain: ChainClient,
  ) {}

  /**
   * Synchronous, and returns before any of the work it starts.
   *
   * Not `async`: the signature allows either, and a plain return makes it
   * impossible to add an `await` here without noticing what that would mean.
   */
  onBlockRange(_from: number, _to: number, signal: AbortSignal): ProcessorOutcome {
    if (this.running === null && !signal.aborted && Date.now() >= this.nextRunAt) {
      // Errors are handled inside `run`; the `void` is the point rather than an
      // oversight, and `finally` is what guarantees the guard is released even
      // if something escapes.
      this.running = this.run(signal).finally(() => {
        this.running = null;
      });
    }

    return ok();
  }

  /**
   * Nothing to undo. A fork does not change what an asset is worth now, and the
   * row is the current price rather than a claim about the range that was rolled
   * back.
   */
  onReorg(): ProcessorOutcome {
    return ok();
  }

  /** Never rejects. A failure is logged and left for the next run. */
  private async run(signal: AbortSignal): Promise<void> {
    const { chainId, spoke, oracle } = this.options;

    try {
      const reserveIds = await this.listings.forSpoke(chainId, spoke);
      if (reserveIds.length === 0) {
        // Not a failure. On a cold start the Spoke's `AddReserve` events have
        // not been folded yet, so there is nothing to price and it is about to
        // become available — which is the short delay, not the long one.
        this.waitFor(this.options.retryMs, 'no reserves registered yet');
        return;
      }
      if (signal.aborted) return;

      // The *head*, not the range's `to`. During a backfill `to` is historical
      // and a full node cannot serve state there, so every call would fail —
      // and a price is a current-value question anyway, so the head is the only
      // block whose answer is worth storing.
      const head = BigInt(await this.chain.getHeadBlockNumber());
      const { prices, failures } = await this.reader.read(oracle, reserveIds, head);

      const rows: ReservePriceRow[] = [...prices].map(([reserveId, price]) => ({
        chainId,
        spoke,
        reserveId,
        price,
      }));

      // Written even on the way out. The reads are already paid for, and
      // throwing them away would mean making them again on the next start.
      await this.store.put(rows);

      if (failures.length > 0) {
        // Named individually: `getReservesPrices` is a batch, so knowing *which*
        // reserve the oracle refused is the difference between one broken feed
        // and an oracle that is down.
        this.waitFor(this.options.retryMs, failures.join('; '));
        return;
      }

      this.logger.log(`priced ${String(rows.length)} reserve(s) at block ${String(head)}`);
      this.nextRunAt = Date.now() + this.options.refreshMs;

      if (signal.aborted) this.logger.log('shutting down; prices will refresh on the next start');
    } catch (error) {
      this.waitFor(this.options.retryMs, error instanceof Error ? error.message : String(error));
    }
  }

  private waitFor(delayMs: number, reason: string): void {
    this.nextRunAt = Date.now() + delayMs;
    this.logger.warn(`pricing incomplete (${reason}); retrying in ${String(delayMs)}ms`);
  }
}
