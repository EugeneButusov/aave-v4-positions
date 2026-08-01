import 'reflect-metadata';

import { NestFactory } from '@nestjs/core';
import { BackfillRunner, type BackfillResult } from '@packages/indexing';
import { Logger } from '@packages/ops';

import { parseBackfillArgs, USAGE } from './backfill/backfill-args';

/** 0 done, 1 refused or failed, 2 bad arguments, 130 interrupted. */
function exitCodeFor(result: BackfillResult): number {
  if (result.kind === 'completed') return 0;
  return result.kind === 'aborted' ? 130 : 1;
}

async function main(): Promise<void> {
  const parsed = parseBackfillArgs(process.argv.slice(2));

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
  const { BackfillModule } = await import('./backfill/backfill.module.js');

  // A context, not an application: nothing listens.
  const context = await NestFactory.createApplicationContext(BackfillModule, { bufferLogs: true });
  const logger = context.get(Logger);
  context.useLogger(logger);
  context.flushLogs();

  const abort = new AbortController();
  const stop = (): void => abort.abort();
  process.once('SIGINT', stop);
  process.once('SIGTERM', stop);

  try {
    const result = await context.get(BackfillRunner).run(parsed.request, abort.signal);

    if (result.kind === 'failed') {
      logger.error(`backfill failed: ${result.reason}`, 'Backfill');
    } else if (result.kind === 'aborted') {
      logger.warn('backfill interrupted', 'Backfill');
    }

    // Only once some of the range actually landed. A run refused up front — a
    // bad processor name, a range above the safe head — has nothing to resume,
    // and offering a `--from` there would imply otherwise.
    if (result.kind !== 'completed' && result.resumeFrom > parsed.request.from) {
      const done = result.resumeFrom - parsed.request.from;
      logger.warn(`${done} block(s) landed; resume with --from ${result.resumeFrom}`, 'Backfill');
    }

    process.exitCode = exitCodeFor(result);
  } finally {
    // Closed rather than exited, so pino flushes what was written above.
    await context.close();
  }
}

void main();
