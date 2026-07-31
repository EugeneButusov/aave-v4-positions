/**
 * The seam every ingestion pipeline plugs into.
 *
 * One source owns one stream of on-chain data — a Spoke's position events, the
 * Hub's asset events, a Chainlink aggregator's `AnswerUpdated` feed. Keeping
 * them behind a single interface is what lets the same process index multiple
 * spokes, hubs or chains from configuration alone.
 *
 * `start` is expected to run until `signal` aborts, and to resolve once it has
 * released everything it holds.
 */
export interface EventSource {
  readonly name: string;
  start(signal: AbortSignal): Promise<void>;
}

/** Multi-provider token. Bind with `{ provide: EVENT_SOURCES, useValue: [...] }`. */
export const EVENT_SOURCES = Symbol('EVENT_SOURCES');
