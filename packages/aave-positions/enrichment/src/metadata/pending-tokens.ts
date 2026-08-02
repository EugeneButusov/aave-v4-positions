import type { Address } from '@packages/indexing';

/**
 * Tokens the Hub has just listed, waiting to be read.
 *
 * The handoff between ingestion and enrichment, and the reason enrichment is
 * not a poll. `AddAsset` is decoded once, by the processor that writes it; the
 * address is in memory at that moment. Without somewhere to put it, the only
 * way for anything else to learn about it is to go back and ask a database
 * every range — which is asking a question whose answer was already known and
 * discarded.
 *
 * **Deliberately in memory, and deliberately not durable.** A process that
 * restarts loses the buffer, and that is fine: the consumer owes a full check
 * on start anyway, because a cold indexer has never read the tokens listed at
 * genesis. Making this survive a restart would be building a second, worse
 * copy of a guarantee that already exists.
 *
 * Not a queue: a *set*. The same token listed twice, or a range re-dispatched
 * after a retry, must not become two reads.
 *
 * **It notifies rather than being polled**, which is what lets the consumer
 * live outside the indexing loop. Buffering an address and then waiting for a
 * block dispatch to notice it would put the loop back in the path — and the
 * loop does not dispatch when it is caught up, stalled, or not started.
 */
export class PendingTokens {
  private readonly tokens = new Set<Address>();

  /** One consumer, so one listener. A second would be two readers of one set. */
  private listener: (() => void) | null = null;

  /**
   * Called from the ingestion path. **Must not throw, and does not** — a
   * listener that fails cannot be allowed to fail the write that decoded the
   * event, so the notification is fire-and-forget and errors stop here.
   */
  add(tokens: readonly Address[]): void {
    const before = this.tokens.size;
    for (const token of tokens) this.tokens.add(token.toLowerCase());
    if (this.tokens.size === before) return;

    try {
      this.listener?.();
    } catch {
      // Swallowed on purpose. The consumer owes a full check on its own
      // schedule, so a missed wake-up costs a delay rather than a token.
    }
  }

  /** Wakes `listener` whenever an address is added that was not already here. */
  notify(listener: () => void): void {
    this.listener = listener;
  }

  /** Whether a consumer has anything to do. Cheap enough to ask every dispatch. */
  get size(): number {
    return this.tokens.size;
  }

  /**
   * Takes everything, leaving the buffer empty.
   *
   * Emptying on read is safe because the consumer's own failure path does not
   * rely on it: a token it could not reach is recovered by the full check its
   * back-off schedules, not by still being in here. Holding them instead would
   * mean two mechanisms for one guarantee, and the buffer growing without a
   * bound whenever a provider is down.
   */
  drain(): readonly Address[] {
    const taken = [...this.tokens];
    this.tokens.clear();
    return taken;
  }
}
