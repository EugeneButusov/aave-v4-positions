import {
  Inject,
  Injectable,
  Logger,
  type OnApplicationBootstrap,
  type OnApplicationShutdown,
} from '@nestjs/common';
import { CHAIN_CLIENT, type Address, type ChainClient } from '@packages/indexing';

import type { ReservePriceRow } from './store/reserve-price';
import { RESERVE_PRICE_STORE, type ReservePriceStore } from './store/reserve-price-store';
import { RESERVE_LISTINGS, type ReserveListings } from './store/reserve-listing-source';
import { RESERVE_PRICE_READER, type ReservePriceReader } from './oracle/reserve-price-reader';

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
  /**
   * Whether to start the timer at all.
   *
   * False for the one-shot command, which builds this graph to reach the ports
   * and must not also acquire a background poller, and for hermetic tests.
   */
  readonly autoStart: boolean;
}

export const RESERVE_PRICE_OPTIONS = Symbol('RESERVE_PRICE_OPTIONS');

/**
 * Keeps what Aave prices each reserve at up to date, on a timer of its own.
 *
 * **Not a `BlockProcessor`, and that is the whole point.** Token metadata is
 * on-chain state that changes on chain, so enrichment can wait for the event
 * that changes it: a range carrying no `AddAsset` cannot have changed the
 * answer, and the indexing loop is a sound trigger. A price is not like that.
 * The oracle's feeds move on their own schedule, off chain, and nothing in the
 * Aave event stream announces one — the answer can be different a second later
 * with no block having said so.
 *
 * Driving this from `onBlockRange` looked equivalent and was not, because the
 * loop does not dispatch on every tick of the wall clock:
 *
 * - **caught up** — `indexRange` is never reached (`next > head` returns
 *   `idle`), so on a quiet chain the prices simply stop refreshing;
 * - **stalled or failed** — the loop stops and prices freeze with it, coupling
 *   two sources with entirely separate failure modes. A provider that cannot
 *   serve `eth_getLogs` over a wide range may serve `eth_call` perfectly well;
 * - **`INDEXER_AUTOSTART=false`** — a pod booted to answer probes would serve
 *   prices that never move.
 *
 * So the trigger is a timer, owned here, started on bootstrap and cancelled on
 * shutdown. `pricing.stale` on the API then measures what it claims to — how
 * long since *this* read — rather than how long since the indexer last found a
 * block.
 *
 * **A self-rescheduling `setTimeout`, never `setInterval`.** The next delay is
 * counted from when a read *finishes*, so a slow provider cannot leave two
 * reads in flight and there is no in-flight latch to get wrong. The two delays
 * say what to do with each outcome: everything resolved waits `refreshMs`,
 * anything left stale waits `retryMs`, which is shorter because §7.1 weighs
 * collateral against debt and one price standing still is worse than all of
 * them ageing together.
 *
 * **Nothing is ever blanked.** A reserve the oracle refused is left out of the
 * write, so its last good price stays put and simply ages.
 */
@Injectable()
export class ReservePriceRefresher implements OnApplicationBootstrap, OnApplicationShutdown {
  private readonly logger = new Logger(ReservePriceRefresher.name);

  /** Fires on shutdown, so a read in flight stops rather than racing `close()`. */
  private readonly abort = new AbortController();

  private timer: NodeJS.Timeout | null = null;

  /** The read in flight, so shutdown can wait for its write rather than cut it. */
  private inFlight: Promise<void> | null = null;

  constructor(
    @Inject(RESERVE_PRICE_OPTIONS) private readonly options: ReservePriceOptions,
    @Inject(RESERVE_LISTINGS) private readonly listings: ReserveListings,
    @Inject(RESERVE_PRICE_STORE) private readonly store: ReservePriceStore,
    @Inject(RESERVE_PRICE_READER) private readonly reader: ReservePriceReader,
    @Inject(CHAIN_CLIENT) private readonly chain: ChainClient,
  ) {}

  onApplicationBootstrap(): void {
    if (!this.options.autoStart) return;
    // Zero, so the first read happens at boot rather than one interval later —
    // otherwise every restart serves no price at all for a minute.
    this.scheduleIn(0);
  }

  async onApplicationShutdown(): Promise<void> {
    this.abort.abort();
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    // Awaited rather than abandoned: a read that has already paid for its
    // `eth_call` should be allowed to store the answer.
    await this.inFlight;
  }

  private scheduleIn(delayMs: number): void {
    if (this.abort.signal.aborted) return;

    this.timer = setTimeout(() => {
      this.inFlight = this.tick().finally(() => {
        this.inFlight = null;
      });
    }, delayMs);

    // The timer must never be a reason for the process to stay alive. Without
    // this, a shutdown that does not reach `onApplicationShutdown` — a command,
    // a failed boot — hangs until the interval elapses.
    this.timer.unref();
  }

  /** Never rejects: a failure picks the shorter delay and comes back. */
  private async tick(): Promise<void> {
    let complete = false;
    try {
      complete = await this.readOnce();
    } catch (error) {
      this.warn(error instanceof Error ? error.message : String(error));
    }

    this.scheduleIn(complete ? this.options.refreshMs : this.options.retryMs);
  }

  /** True when every listed reserve came back with a price. */
  private async readOnce(): Promise<boolean> {
    const { chainId, spoke, oracle } = this.options;

    const reserveIds = await this.listings.forSpoke(chainId, spoke);
    if (reserveIds.length === 0) {
      // Not a failure. On a cold start the Spoke's `AddReserve` events have not
      // been folded yet, so there is nothing to price and it is about to become
      // available — which is the short delay, not the long one.
      this.warn('no reserves registered yet');
      return false;
    }
    if (this.abort.signal.aborted) return false;

    // The *head*, and one block for the whole batch. A price is a current-value
    // question, and pinning is what stops fourteen reads landing at up to
    // fourteen heights across a provider failover list.
    const head = BigInt(await this.chain.getHeadBlockNumber());
    const { prices, failures } = await this.reader.read(oracle, reserveIds, head);

    const rows: ReservePriceRow[] = [...prices].map(([reserveId, price]) => ({
      chainId,
      spoke,
      reserveId,
      price,
    }));

    // Written even on the way out. The reads are already paid for, and throwing
    // them away would mean making them again on the next start.
    await this.store.put(rows);

    if (failures.length > 0) {
      // Named individually: `getReservesPrices` is a batch, so knowing *which*
      // reserve the oracle refused is the difference between one broken feed
      // and an oracle that is down.
      this.warn(failures.join('; '));
      return false;
    }

    this.logger.log(`priced ${String(rows.length)} reserve(s) at block ${String(head)}`);
    return true;
  }

  private warn(reason: string): void {
    this.logger.warn(
      `pricing incomplete (${reason}); retrying in ${String(this.options.retryMs)}ms`,
    );
  }
}
