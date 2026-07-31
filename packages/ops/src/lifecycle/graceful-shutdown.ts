import { Logger, type INestApplication } from '@nestjs/common';

import { HealthService } from '../health/health.service';

const SIGNALS = ['SIGTERM', 'SIGINT'] as const;

/**
 * Drains the pod in the order Kubernetes expects.
 *
 * `app.enableShutdownHooks()` closes the server as soon as SIGTERM lands, which
 * races the endpoints controller: the pod can still be receiving traffic that
 * it is no longer listening for. Instead we fail readiness first, hold for the
 * grace window so the removal propagates, and only then close.
 */
export function installGracefulShutdown(
  app: INestApplication,
  options: { graceSeconds: number },
): void {
  const logger = new Logger('Shutdown');
  const health = app.get(HealthService);
  let started = false;

  const drain = async (signal: NodeJS.Signals): Promise<void> => {
    if (started) return;
    started = true;

    logger.log(`${signal} received; failing readiness for ${options.graceSeconds}s before close`);
    health.beginShutdown();

    await new Promise((resolve) => setTimeout(resolve, options.graceSeconds * 1_000));

    try {
      await app.close();
      logger.log('shutdown complete');
      process.exit(0);
    } catch (error) {
      logger.error('shutdown failed', error instanceof Error ? error.stack : String(error));
      process.exit(1);
    }
  };

  for (const signal of SIGNALS) {
    process.once(signal, (received: NodeJS.Signals) => void drain(received));
  }
}
