/**
 * Everything the indexing loop and its collaborators are configured with,
 * resolved once at module setup.
 */
export interface IndexingOptions {
  /**
   * The chain we expect to be indexing. Checked against what the providers
   * actually report on the first iteration — pointing an indexer at the wrong
   * chain yields plausible, wrong data rather than an error, so it is worth one
   * RPC call to rule out.
   */
  readonly chainId: number;

  /** Tried in order; the first is preferred, not merely one of several. */
  readonly rpcUrls: readonly string[];
  readonly rpcTimeoutMs: number;

  /**
   * How deep a reorg is assumed to be able to reach. Consumed by the reorg
   * detector — the loop never reads it, and asks
   * {@link ReorgDetector.safeHead} instead.
   */
  readonly finalityDepth: number;

  /** Where to begin when the cursor store has nothing for this chain. */
  readonly startBlock: number;

  /**
   * Upper bound on the width of one dispatched range while catching up. Halved
   * at runtime whenever a processor answers `retry` with `narrowRange`.
   */
  readonly maxRangeSize: number;

  /** How long to wait after reaching the tip before looking for a new head. */
  readonly pollIntervalMs: number;

  /** How long the loop may make no progress before readiness starts failing. */
  readonly stallThresholdMs: number;

  /** `false` in tests, so importing the module does not start a loop. */
  readonly autoStart: boolean;
}

export const INDEXING_OPTIONS = Symbol('INDEXING_OPTIONS');

/**
 * Backoff between retried iterations: exponential from `RETRY_BASE_MS`, capped
 * at `RETRY_MAX_MS`. Constants rather than configuration — retries are
 * unbounded and the stall alarm, not the delay, is what makes a wedged loop
 * visible, so there is nothing an operator would usefully tune here.
 */
export const RETRY_BASE_MS = 500;
export const RETRY_MAX_MS = 30_000;

/**
 * How long shutdown waits for an in-flight dispatch before abandoning it. The
 * loop aborts its signal first, so a processor that honours it finishes well
 * inside this; the bound only exists so one that ignores it cannot make
 * `app.close()` outlive the container's termination grace period.
 */
export const SHUTDOWN_DRAIN_MS = 10_000;
