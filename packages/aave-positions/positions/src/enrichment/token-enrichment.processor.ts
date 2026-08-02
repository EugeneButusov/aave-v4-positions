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
  /** How often the full sweep runs. The fast path runs every dispatch. */
  readonly sweepIntervalMs: number;
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
 * A `BlockProcessor` rather than a new scheduling concept: it inherits the
 * loop's lifecycle, its `AbortSignal` and its graceful drain, and the indexer is
 * already the thing that runs continuously.
 *
 * **Two mechanisms, and only one of them is load-bearing.**
 *
 * The *fast path* asks the Hub ledger which tokens an `AddAsset` named inside
 * this range. It is a granule-pruned seek — measured at 1 granule of 4 over
 * 60,000 hub events — and costs no RPC, because the Hub processor wrote the row
 * earlier in the same dispatch. A new listing is enriched in the dispatch that
 * ingested it.
 *
 * The *sweep* diffs the full listed-versus-stored sets on a timer. It is what
 * makes bootstrap work at all: every `AddAsset` on mainnet fired at block
 * 24,722,784, far behind any live cursor, so the fast path alone would discover
 * nothing on a fresh start. The latch begins expired so the first dispatch
 * sweeps.
 *
 * The sweep is also what lets the fast path stay cheap. It depends on running
 * after the Hub processor, and `dispatch.ts` plans to drop that guarantee — so
 * a missed arrival, a reordering, or a fetch against a flaky node degrades to
 * "up to one sweep late" rather than "silently never". The fast path is an
 * optimisation over the sweep, not a mechanism anything depends on.
 *
 * **It always returns `ok()`, and that is correct rather than a compromise.** A
 * third-party token contract must never stall Aave ingestion, and a `retry`
 * would do exactly that. Swallowing is safe *because* discovery is gap-driven
 * and idempotent: a failure leaves the gap open and the next sweep retries it.
 */
@Injectable()
export class TokenEnrichmentProcessor implements BlockProcessor {
  readonly name = 'token-enrichment';

  private readonly logger = new Logger(TokenEnrichmentProcessor.name);

  /** Epoch ms of the last sweep. Zero so the first dispatch always sweeps. */
  private lastSweepAt = 0;

  constructor(
    @Inject(TOKEN_ENRICHMENT_OPTIONS) private readonly options: TokenEnrichmentOptions,
    @Inject(TOKEN_LISTINGS) private readonly listings: TokenListings,
    @Inject(TOKEN_METADATA_STORE) private readonly store: TokenMetadataStore,
    @Inject(ERC20_METADATA_READER) private readonly reader: Erc20MetadataReader,
    @Inject(CHAIN_CLIENT) private readonly chain: ChainClient,
  ) {}

  async onBlockRange(from: number, to: number, signal: AbortSignal): Promise<ProcessorOutcome> {
    try {
      const wanted = await this.discover(from, to);
      if (wanted.length > 0 && !signal.aborted) await this.enrich(wanted);
    } catch (error) {
      // Logged and dropped. The gap is still open, so the next sweep tries
      // again — and Aave ingestion does not stop because a token contract or a
      // provider misbehaved.
      this.logger.warn(
        `enrichment skipped: ${error instanceof Error ? error.message : String(error)}`,
      );
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

  /** The union of the fast path and, when due, the sweep — minus what is stored. */
  private async discover(from: number, to: number): Promise<readonly Address[]> {
    const due = Date.now() - this.lastSweepAt >= this.options.sweepIntervalMs;
    const listed = due
      ? await this.listings.all(this.options.chainId)
      : await this.listings.addedIn(this.options.chainId, from, to);

    // Stamped before the work, not after. A sweep that takes longer than the
    // interval would otherwise start again the moment it finished.
    if (due) this.lastSweepAt = Date.now();
    if (listed.length === 0) return [];

    const known = await this.store.labels(this.options.chainId);
    return [...new Set(listed.map((token) => token.toLowerCase()))].filter(
      (token) => !known.has(token),
    );
  }

  private async enrich(tokens: readonly Address[]): Promise<void> {
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
      // being re-read on every sweep forever. But only when the token actually
      // answered: a timeout must leave the gap open rather than close it on a
      // null nobody will revisit.
      if (!answered(metadata)) {
        this.logger.warn(
          `${token}: unreachable, leaving the gap open (${metadata.failures.join(', ')})`,
        );
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

    await this.store.put(rows);
    if (rows.length > 0) this.logger.log(`enriched ${rows.length} token(s) at block ${head}`);
  }
}
