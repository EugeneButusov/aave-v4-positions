import 'reflect-metadata';

import { NestFactory } from '@nestjs/core';
import { ConfigService } from '@nestjs/config';
import { Logger } from '@packages/ops';

import { parseEnrichArgs, USAGE } from './enrich/enrich-args';
import type { Env } from './config/env';

async function main(): Promise<void> {
  const parsed = parseEnrichArgs(process.argv.slice(2));

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
  const { EnrichModule } = await import('./enrich/enrich.module.js');
  const { TokenEnricher } = await import('./enrich/token-enricher.js');

  const context = await NestFactory.createApplicationContext(EnrichModule, { bufferLogs: true });
  const logger = context.get(Logger);
  context.useLogger(logger);
  context.flushLogs();

  const abort = new AbortController();
  const stop = (): void => abort.abort();
  process.once('SIGINT', stop);
  process.once('SIGTERM', stop);

  try {
    const chainId = context.get(ConfigService<Env, true>).get('CHAIN_ID', { infer: true });
    const result = await context.get(TokenEnricher).run(chainId, parsed.request);

    logger.log(
      `asked ${result.asked}, resolved ${result.resolved}, unreachable ${result.unreachable.length}`,
      'Enrich',
    );

    // Non-zero on a remaining gap, so this can gate a deploy step rather than
    // only inform one.
    if (result.unreachable.length > 0) {
      logger.error(`still unenriched: ${result.unreachable.join(', ')}`, undefined, 'Enrich');
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
