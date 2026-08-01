/**
 * What a processor reports back after handling a dispatch.
 *
 * Explicit rather than throw-to-fail, because the loop needs to distinguish
 * "try me again" from "stop, a human is required" — and an exception cannot
 * carry that distinction without a bespoke error hierarchy.
 */
export type ProcessorOutcome =
  | { readonly status: 'ok' }
  /**
   * Transient. The cursor holds, the loop backs off and re-dispatches the same
   * range. Retries are unbounded: an RPC outage should recover by itself, and a
   * stuck processor is surfaced by the stall alarm on `/health/ready` rather
   * than by giving up.
   */
  | { readonly status: 'retry'; readonly reason: string; readonly narrowRange?: boolean }
  /**
   * Terminal. Reserved for "more attempts cannot help" — a malformed
   * configuration, a schema the processor cannot parse. The loop stops and
   * readiness fails.
   */
  | { readonly status: 'failed'; readonly reason: string };

export function ok(): ProcessorOutcome {
  return { status: 'ok' };
}

/**
 * `narrowRange` asks the loop to halve its range before re-dispatching. Return
 * it when a provider rejects the request for being too wide: measured caps
 * across public endpoints run from 50 to 10,000 blocks (analysis §8), so a
 * range that works against one provider fails against another, and without
 * this the loop would re-send the identical oversized range forever.
 */
export function retry(reason: string, options?: { narrowRange?: boolean }): ProcessorOutcome {
  return { status: 'retry', reason, ...(options?.narrowRange === true && { narrowRange: true }) };
}

export function failed(reason: string): ProcessorOutcome {
  return { status: 'failed', reason };
}

/**
 * A unit of indexing work. Processors are the only place domain knowledge
 * lives; the loop knows about block numbers and nothing else.
 *
 * **Processors must be idempotent.** Dispatch is at-least-once, per processor,
 * per range. The loop runs processors in registration order and stops at the
 * first non-`ok`, so a processor that already returned `ok` is re-invoked for
 * the same range when a later one asks to retry. It is also re-invoked after a
 * crash between a processor committing its own writes and the cursor advancing.
 * This is not a wart to be engineered away: positions are a fold over an
 * immutable event log, so replay is already the repair primitive (analysis §8,
 * §9.4).
 *
 * Both methods receive an `AbortSignal` that fires when the process is shutting
 * down. Honouring it is what keeps a long range from outliving the container's
 * termination grace period; ignoring it is safe but wastes the work.
 *
 * Returning `T | Promise<T>` rather than `Promise<T>` lets a synchronous
 * processor stay synchronous — `require-await` rejects an `async` method with
 * nothing to await.
 */
export interface BlockProcessor {
  readonly name: string;

  /**
   * Handle every block in `[from, to]`, inclusive. Near the chain tip the loop
   * dispatches one block at a time, so `from === to`; while catching up it
   * dispatches wider ranges, which is what makes a backfill of ~932k blocks
   * finish in minutes rather than days.
   */
  onBlockRange(
    from: number,
    to: number,
    signal: AbortSignal,
  ): ProcessorOutcome | Promise<ProcessorOutcome>;

  /**
   * Discard everything derived from blocks `[firstInvalidBlock,
   * lastInvalidBlock]`, inclusive — they were on a branch that lost. The loop
   * rewinds its cursor to `firstInvalidBlock - 1` afterwards and re-dispatches
   * the replacement blocks through {@link onBlockRange}.
   */
  onReorg(
    firstInvalidBlock: number,
    lastInvalidBlock: number,
    signal: AbortSignal,
  ): ProcessorOutcome | Promise<ProcessorOutcome>;
}

/**
 * Multi-provider token holding the registered processors in order. Bound by
 * `IndexingModule.forRootAsync({ processors: [...] })`.
 */
export const BLOCK_PROCESSORS = Symbol('BLOCK_PROCESSORS');
