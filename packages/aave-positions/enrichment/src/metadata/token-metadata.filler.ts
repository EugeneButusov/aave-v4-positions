import {
  Inject,
  Injectable,
  Logger,
  type OnApplicationBootstrap,
  type OnApplicationShutdown,
} from '@nestjs/common';
import {
  CHAIN_CLIENT,
  ERC20_METADATA_READER,
  type Address,
  type ChainClient,
  type Erc20MetadataReader,
  type TokenMetadata,
} from '@packages/indexing';

import { TOKEN_LISTINGS, type TokenListings } from '../store/token-listing-source';
import { PendingTokens } from './pending-tokens';
import type { TokenMetadataRow } from '../store/token-metadata';
import { TOKEN_METADATA_STORE, type TokenMetadataStore } from '../store/token-metadata-store';

export interface TokenMetadataOptions {
  readonly chainId: number;
  /** How long to wait before trying again after a run left a gap open. */
  readonly retryDelayMs: number;
  readonly concurrency: number;
  /**
   * Whether to do anything at all.
   *
   * False for the one-shot command, which builds this graph to reach the ports
   * and must not also acquire a background filler, and for hermetic tests.
   */
  readonly autoStart: boolean;
}

export const TOKEN_METADATA_OPTIONS = Symbol('TOKEN_METADATA_OPTIONS');

/**
 * Failures that mean **the token answered** — so a null is its final answer and
 * the row should be written.
 *
 * Everything else is us failing to ask: a timeout, a 429, a provider that is
 * down. Writing a row for one of those would close the gap on a false null and
 * the token would never be read again, which is the exact failure the
 * write-even-when-null rule exists to avoid in the other direction.
 *
 * An unrecognised name is treated as *not* answered, so a new viem error class
 * costs a retry rather than a permanently blank label.
 */
const ANSWERED = new Set([
  // The contract ran and rejected the call.
  'ContractFunctionRevertedError',
  // Nothing came back: no code at the address, or no such method.
  'ContractFunctionZeroDataError',
  // Something came back and could not be decoded either way — a lying or
  // truncated payload. The token answered; the answer is unusable.
  'IntegerOutOfRangeError',
  'PositionOutOfBoundsError',
  'AbiDecodingZeroDataError',
]);

/** `symbol: ContractFunctionRevertedError` -> `ContractFunctionRevertedError`. */
function classify(failure: string): string {
  return failure.slice(failure.indexOf(':') + 1).trim();
}

/**
 * Whether a read is a final answer about the token rather than a failure to
 * reach it.
 *
 * `decimals: out of range (999)` is our own rejection of a value the token did
 * give us, so it counts as answered.
 */
function answered(metadata: TokenMetadata): boolean {
  return metadata.failures.every(
    (failure) => ANSWERED.has(classify(failure)) || failure.includes('out of range'),
  );
}

/** Runs `work` over `items`, at most `limit` at a time, never rejecting. */
async function mapLimit<T, R>(
  items: readonly T[],
  limit: number,
  work: (item: T) => Promise<R>,
): Promise<PromiseSettledResult<R>[]> {
  const results: PromiseSettledResult<R>[] = [];
  for (let start = 0; start < items.length; start += limit) {
    const batch = items.slice(start, start + limit);
    // oxlint-disable-next-line no-await-in-loop
    results.push(...(await Promise.allSettled(batch.map(work))));
  }
  return results;
}

/**
 * Fills in what tokens call themselves, automatically.
 *
 * **Driven by the listing, not by the loop.** `AddAsset` is the only event that
 * can change which tokens are listed — the Hub has no delisting event at all,
 * and `Remove` is a liquidity withdrawal (§4.5) — and the Hub processor decodes
 * it, address and all, as it writes it. That address goes straight into
 * {@link PendingTokens}, which *wakes this*: no query, no Postgres, no chain,
 * and no schedule anybody had to tune. Going back to a database every range to
 * rediscover a token the process already had in hand was the thing to remove.
 *
 * **But it is not a `BlockProcessor`, and that part is deliberate.** It used to
 * be, and the trigger was a dispatch — which put the indexing loop in the path
 * of three things that have nothing to do with it:
 *
 * - the **initial full check** waited for the first dispatch, so a pod booted
 *   with `INDEXER_AUTOSTART=false` never read a single token;
 * - a **retry** waited for one too, so a provider outage that outlasted the
 *   chain's next block left a token unlabelled until the indexer moved again —
 *   and if the indexer had stalled or failed, forever;
 * - a **push** sat in the buffer until something unrelated happened.
 *
 * None of that is what the loop is for, and an ERC-20 read has no more to do
 * with a block range than an oracle read does. So the wake-up comes from the
 * buffer and the retry comes from a timer this class owns.
 *
 * Two cases still need the whole listing set, and both are *states* rather
 * than a schedule:
 *
 * - **nothing has been checked yet.** Every `AddAsset` on mainnet fired at
 *   block 24,722,784, far behind any live cursor, so a freshly started indexer
 *   is never pushed the tokens it has never read. This is also what covers the
 *   buffer being in memory: a restart loses it, and the full check it already
 *   owes on start is the recovery.
 * - **the last run left a gap open.** The addresses it failed on have been
 *   drained and will not be pushed again, so the retry has to re-derive them.
 *
 * One flag covers both. A clean run arms **no timer at all** — it sleeps until
 * something is listed, which is the property the push was introduced for.
 *
 * The work is safe to skip, interrupt or lose because it is gap-driven and
 * idempotent: what to do next comes from the difference between two tables, so
 * a run lost to a crash costs nothing.
 */
@Injectable()
export class TokenMetadataFiller implements OnApplicationBootstrap, OnApplicationShutdown {
  private readonly logger = new Logger(TokenMetadataFiller.name);

  /** Fires on shutdown, so a read in flight stops rather than racing `close()`. */
  private readonly abort = new AbortController();

  /** The run in flight, if any. Its only job is to stop a second one starting. */
  private running: Promise<void> | null = null;

  /** Armed only after a run that left a gap. A clean run schedules nothing. */
  private retry: NodeJS.Timeout | null = null;

  /**
   * Whether the next run has to ask for the whole listing set.
   *
   * True at construction — nothing has been checked yet — and true again after
   * any run that left a gap open, because the addresses it failed on will not
   * appear in a range it is handed later. False otherwise, which is the case
   * that makes the usual dispatch cost one seek.
   */
  private needsFullCheck = true;

  constructor(
    @Inject(TOKEN_METADATA_OPTIONS) private readonly options: TokenMetadataOptions,
    @Inject(TOKEN_LISTINGS) private readonly listings: TokenListings,
    private readonly pending: PendingTokens,
    @Inject(TOKEN_METADATA_STORE) private readonly store: TokenMetadataStore,
    @Inject(ERC20_METADATA_READER) private readonly reader: Erc20MetadataReader,
    @Inject(CHAIN_CLIENT) private readonly chain: ChainClient,
  ) {}

  onApplicationBootstrap(): void {
    if (!this.options.autoStart) return;

    // Subscribed before the first run, so a listing that lands mid-check is a
    // wake-up rather than a token nobody comes back for.
    this.pending.notify(() => {
      this.wake();
    });
    this.wake();
  }

  async onApplicationShutdown(): Promise<void> {
    this.abort.abort();
    if (this.retry !== null) {
      clearTimeout(this.retry);
      this.retry = null;
    }
    // Awaited rather than abandoned: reads already paid for should be stored.
    await this.running;
  }

  /**
   * Starts a run unless one is already going.
   *
   * Returns before any of the work — a listing must never wait on seventeen
   * ERC-20 reads to finish being recorded, and `add()` is called from the
   * ingestion path.
   */
  private wake(): void {
    if (this.running !== null || this.abort.signal.aborted) return;

    // **A listing does not cut short a back-off.** The armed retry re-derives
    // the whole set, so it already covers whatever was just pushed — and
    // without this, a provider that is down would be hit again by every
    // `AddAsset` rather than on the schedule the back-off exists to impose.
    if (this.retry !== null) return;

    // Errors are handled inside `run`; the `void` is the point rather than an
    // oversight, and `finally` is what guarantees the guard is released even
    // if something escapes.
    this.running = this.run(this.abort.signal).finally(() => {
      this.running = null;
    });
  }

  /** Never rejects. A failure is logged and left for the next run to retry. */
  private async run(signal: AbortSignal): Promise<void> {
    try {
      const missing = await this.missing();
      if (missing.length === 0) {
        // Nothing outstanding, so whatever made the last run ask for everything
        // is settled. The next dispatch costs one seek.
        this.needsFullCheck = false;
        return;
      }
      if (signal.aborted) return;

      const remaining = await this.enrich(missing, signal);
      if (remaining > 0) {
        this.backOff(`${remaining} token(s) still unresolved`);
      } else {
        this.needsFullCheck = false;
      }
    } catch (error) {
      this.backOff(error instanceof Error ? error.message : String(error));
    }
  }

  private backOff(reason: string): void {
    // The addresses this run failed on have been drained and will not be pushed
    // again, so the retry has to go back to the whole set to find them.
    this.needsFullCheck = true;
    this.logger.warn(
      `enrichment incomplete (${reason}); retrying in ${this.options.retryDelayMs}ms`,
    );

    if (this.abort.signal.aborted) return;
    this.retry = setTimeout(() => {
      this.retry = null;
      this.wake();
    }, this.options.retryDelayMs);
    // Never a reason for the process to stay alive.
    this.retry.unref();
  }

  /**
   * Listed on the Hub, absent from the store.
   *
   * The usual answer needs no query at all — the addresses were handed over as
   * they were ingested. Draining unconditionally matters even when a full check
   * is owed: leaving them buffered would wake the next dispatch for work this
   * one has already covered.
   */
  private async missing(): Promise<readonly Address[]> {
    const pushed = this.pending.drain();
    const listed = this.needsFullCheck ? await this.listings.all(this.options.chainId) : pushed;

    if (listed.length === 0) return [];

    const known = await this.store.labels(this.options.chainId);
    return [...new Set(listed.map((token) => token.toLowerCase()))].filter(
      (token) => !known.has(token),
    );
  }

  /** Reads and stores what it can, and reports how many gaps are still open. */
  private async enrich(tokens: readonly Address[], signal: AbortSignal): Promise<number> {
    // One block for the whole batch, and the *head* rather than the range's
    // `to`. During a backfill `to` is historical, and a full node cannot serve
    // state there — every call would fail and, worse, fail in a way that looks
    // like a token with no symbol. Metadata is immutable, so reading it at the
    // head is both correct and the thing a full node can answer.
    const head = BigInt(await this.chain.getHeadBlockNumber());

    const settled = await mapLimit(tokens, this.options.concurrency, async (token) => ({
      token,
      metadata: await this.reader.read(token, head),
    }));

    const rows: TokenMetadataRow[] = [];
    for (const result of settled) {
      if (result.status === 'rejected') continue;
      const { token, metadata } = result.value;

      // A row is written even when every field is null — that is what records
      // that the question was put, and what stops a token with no `symbol()`
      // being re-read on every run forever. But only when the token actually
      // answered: a timeout must leave the gap open rather than close it on a
      // null nobody will revisit.
      if (!answered(metadata)) {
        this.logger.warn(`${token}: unreachable (${metadata.failures.join(', ')})`);
        continue;
      }

      rows.push({
        chainId: this.options.chainId,
        token,
        symbol: metadata.symbol,
        name: metadata.name,
        tokenDecimals: metadata.decimals,
        fetchedAtBlock: Number(head),
      });
    }

    // Written even on the way out. The reads are already paid for, and throwing
    // them away would mean doing them again on the next start.
    await this.store.put(rows);
    if (rows.length > 0) this.logger.log(`enriched ${rows.length} token(s) at block ${head}`);
    if (signal.aborted) this.logger.log('shutting down; enrichment will resume from the gap');

    return tokens.length - rows.length;
  }
}
