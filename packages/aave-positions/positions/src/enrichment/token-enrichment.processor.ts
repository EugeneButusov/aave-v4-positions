import { Inject, Injectable, Logger } from '@nestjs/common';
import {
  CHAIN_CLIENT,
  ERC20_METADATA_READER,
  ok,
  type Address,
  type BlockProcessor,
  type ChainClient,
  type Erc20MetadataReader,
  type ProcessorOutcome,
  type TokenMetadata,
} from '@packages/indexing';

import { TOKEN_LISTINGS, type TokenListings } from '../store/token-listing-source';
import type { TokenMetadataRow } from '../store/token-metadata';
import { TOKEN_METADATA_STORE, type TokenMetadataStore } from '../store/token-metadata-store';

export interface TokenEnrichmentOptions {
  readonly chainId: number;
  /** How long to wait before trying again after a run left a gap open. */
  readonly retryDelayMs: number;
  readonly concurrency: number;
}

export const TOKEN_ENRICHMENT_OPTIONS = Symbol('TOKEN_ENRICHMENT_OPTIONS');

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
 * **`onBlockRange` awaits nothing and never blocks the loop.** That is the
 * whole shape of this class, and it is not a detail. Enrichment reads
 * third-party token contracts — seventeen of them, three calls each, against a
 * provider that may be slow, rate-limiting or down — and `dispatchToProcessors`
 * runs processors one after another. Awaiting that work here would hold up the
 * Spoke and Hub ledgers behind an ERC-20, which inverts what matters: the
 * indexer staying in step with the chain is the job, and a token symbol is a
 * nice-to-have that can be minutes late without anyone noticing.
 *
 * So a dispatch **triggers** a run and returns immediately. At most one runs at
 * a time, and a run that leaves a gap open backs off before the next attempt,
 * so a dead provider is retried on a timer rather than on every block.
 *
 * That works because discovery is gap-driven and idempotent: what to do next is
 * derived from the difference between two tables, never from what this dispatch
 * happened to see. A run that is skipped, interrupted mid-flight by a shutdown,
 * or lost to a crash costs nothing — the gap is still there, and the next run
 * finds it. It is also why there is no fast-path/sweep split any more. That
 * split existed to keep an expensive listing query off the hot path — and the
 * query turned out not to be expensive: it reads 34 rows in 3 ms whether the
 * fold holds 29,631 rows or three million, because `underlying` is sparse. With
 * nothing to avoid, the cheap-but-narrow second path had nothing left to buy.
 */
@Injectable()
export class TokenEnrichmentProcessor implements BlockProcessor {
  readonly name = 'token-enrichment';

  private readonly logger = new Logger(TokenEnrichmentProcessor.name);

  /** The run in flight, if any. Its only job is to stop a second one starting. */
  private running: Promise<void> | null = null;

  /** Epoch ms before which not to try again, after a run left a gap open. */
  private retryAfter = 0;

  constructor(
    @Inject(TOKEN_ENRICHMENT_OPTIONS) private readonly options: TokenEnrichmentOptions,
    @Inject(TOKEN_LISTINGS) private readonly listings: TokenListings,
    @Inject(TOKEN_METADATA_STORE) private readonly store: TokenMetadataStore,
    @Inject(ERC20_METADATA_READER) private readonly reader: Erc20MetadataReader,
    @Inject(CHAIN_CLIENT) private readonly chain: ChainClient,
  ) {}

  /**
   * Synchronous, and returns before any of the work it starts.
   *
   * Not `async`: the signature allows either, and a plain return makes it
   * impossible to add an `await` here without noticing what that would mean.
   */
  onBlockRange(_from: number, _to: number, signal: AbortSignal): ProcessorOutcome {
    if (this.running === null && !signal.aborted && Date.now() >= this.retryAfter) {
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
   * Nothing to undo. A fork does not unmint an ERC-20, and a listing that is
   * rolled back leaves a metadata row nothing joins to — orphaned, not wrong.
   */
  onReorg(): ProcessorOutcome {
    return ok();
  }

  /** Never rejects. A failure is logged and left for the next run to retry. */
  private async run(signal: AbortSignal): Promise<void> {
    try {
      const missing = await this.missing();
      if (missing.length === 0 || signal.aborted) return;

      const remaining = await this.enrich(missing, signal);
      if (remaining > 0) this.backOff(`${remaining} token(s) still unresolved`);
    } catch (error) {
      this.backOff(error instanceof Error ? error.message : String(error));
    }
  }

  private backOff(reason: string): void {
    this.retryAfter = Date.now() + this.options.retryDelayMs;
    this.logger.warn(
      `enrichment incomplete (${reason}); retrying in ${this.options.retryDelayMs}ms`,
    );
  }

  /** Listed on the Hub, absent from the store. */
  private async missing(): Promise<readonly Address[]> {
    const listed = await this.listings.all(this.options.chainId);
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
