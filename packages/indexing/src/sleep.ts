/**
 * Waits `ms`, or returns early when `signal` fires — so a loop that is idling
 * or backing off does not sit out its full delay before noticing a shutdown.
 *
 * Composes the two signals rather than racing a timer against a listener: there
 * is one wake path, so there is no cleanup to get wrong and no way to settle
 * twice. The timeout signal does not hold the event loop open.
 */
export function sleepUntil(ms: number, signal: AbortSignal): Promise<void> {
  if (ms <= 0 || signal.aborted) return Promise.resolve();

  const wake = AbortSignal.any([signal, AbortSignal.timeout(ms)]);
  return new Promise<void>((resolve) => {
    wake.addEventListener('abort', () => resolve(), { once: true });
  });
}
