import type { BlockProcessor, ProcessorOutcome } from './block-processor';

/**
 * What a whole dispatch amounted to, as opposed to {@link ProcessorOutcome},
 * which is one processor's answer.
 *
 * Three differences, all so the caller does not have to re-derive them:
 * `narrowRange` is normalised from optional to a plain boolean, `reason` is
 * already composed for a human, and `processor` names who decided it.
 */
export type DispatchResult =
  | { readonly status: 'ok' }
  | {
      readonly status: 'retry';
      readonly processor: string;
      readonly reason: string;
      readonly narrowRange: boolean;
    }
  | { readonly status: 'failed'; readonly processor: string; readonly reason: string };

export function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Runs the processors in registration order, stopping at the first non-`ok`.
 *
 * Fail-fast means an earlier processor is re-invoked when a later one asks to
 * retry, which is why the interface requires idempotence.
 *
 * Shared by the indexing loop and the backfill runner. They differ in what they
 * do with the answer — one backs off forever, the other gives up after a bounded
 * number of attempts — but not in how a range is handed to processors, and a
 * second copy of that is exactly what would drift.
 *
 * TODO: parallelize once the real processors land — the Spoke, Hub and price
 * streams have no ordering dependency during backfill (analysis §8). Needs
 * `allSettled`, not `all`: outcomes are returned rather than thrown, so
 * `Promise.all` cannot fail fast here, and it does not cancel its siblings
 * either. Also needs a concurrency cap for the measured provider rate limits,
 * `narrowRange` reported once per dispatch rather than once per processor that
 * asks, and registration order dropped from the {@link BlockProcessor} contract.
 */
export async function dispatchToProcessors(
  processors: readonly BlockProcessor[],
  signal: AbortSignal,
  invoke: (
    processor: BlockProcessor,
    signal: AbortSignal,
  ) => ProcessorOutcome | Promise<ProcessorOutcome>,
): Promise<DispatchResult> {
  for (const processor of processors) {
    let outcome: ProcessorOutcome;
    try {
      // oxlint-disable-next-line no-await-in-loop
      outcome = await invoke(processor, signal);
    } catch (error) {
      // A thrown error is a retry, not a failure. It is indistinguishable from
      // one at this level, and treating it as terminal would let a transient
      // bug stop indexing permanently.
      return {
        status: 'retry',
        processor: processor.name,
        reason: `${processor.name} threw: ${describeError(error)}`,
        narrowRange: false,
      };
    }

    if (outcome.status === 'failed') {
      return {
        status: 'failed',
        processor: processor.name,
        reason: `${processor.name}: ${outcome.reason}`,
      };
    }

    if (outcome.status === 'retry') {
      return {
        status: 'retry',
        processor: processor.name,
        reason: `${processor.name}: ${outcome.reason}`,
        narrowRange: outcome.narrowRange === true,
      };
    }
  }

  return { status: 'ok' };
}
