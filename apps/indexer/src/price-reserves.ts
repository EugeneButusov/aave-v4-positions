import 'reflect-metadata';

import { NestFactory } from '@nestjs/core';
import { ConfigService } from '@nestjs/config';
import { Logger } from '@packages/ops';

import { parsePriceArgs, USAGE } from './price/price-args';
import type { Env } from './config/env';

async function main(): Promise<void> {
  const parsed = parsePriceArgs(process.argv.slice(2));

  // Written directly rather than logged: this is help text, it precedes the
  // logger existing, and `no-console` leaves only `warn` and `error` anyway.
  if (parsed.kind === 'help') {
    process.stdout.write(USAGE);
    return;
  }

  if (parsed.kind === 'invalid') {
    process.stderr.write(`${parsed.reason}\n\n${USAGE}`);
    process.exitCode = 2;
    return;
  }

  // Loaded here rather than at the top of the file: the module graph validates
  // the environment as it is constructed, and `--help` has to answer on a
  // machine with no chain configured.
  // The `.js` is required: `nodenext` resolves a dynamic import by ESM rules
  // even from a CommonJS file, and those want the emitted extension.
  const { PriceModule } = await import('./price/price.module.js');
  const { ReservePricer } = await import('./price/reserve-pricer.js');

  const context = await NestFactory.createApplicationContext(PriceModule, { bufferLogs: true });
  const logger = context.get(Logger);
  context.useLogger(logger);
  context.flushLogs();

  const abort = new AbortController();
  const stop = (): void => abort.abort();
  process.once('SIGINT', stop);
  process.once('SIGTERM', stop);

  try {
    const config = context.get(ConfigService<Env, true>);
    const result = await context
      .get(ReservePricer)
      .run(
        config.get('CHAIN_ID', { infer: true }),
        config.get('MAIN_SPOKE_ADDRESS', { infer: true }),
        config.get('MAIN_SPOKE_ORACLE_ADDRESS', { infer: true }),
        parsed.request,
      );

    logger.log(
      `asked ${result.asked}, priced ${result.priced}, unpriced ${result.unpriced.length}`,
      'Price',
    );

    // Non-zero on a reserve the oracle would not price, so this can gate a
    // deploy step rather than only inform one — an unpriced reserve is a null
    // USD value on every position in it.
    if (result.unpriced.length > 0) {
      logger.error(`still unpriced: ${result.unpriced.join(', ')}`, undefined, 'Price');
      process.exitCode = 1;
    } else if (abort.signal.aborted) {
      process.exitCode = 130;
    }
  } finally {
    // Closed rather than exited, so pino flushes what was written above.
    await context.close();
  }
}

void main().catch((error: unknown) => {
  process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`);
  process.exitCode = 1;
});
