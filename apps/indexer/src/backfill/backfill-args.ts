import { parseArgs } from 'node:util';

import type { BackfillRequest } from '@packages/indexing';
import { z } from 'zod';

export const USAGE = `Usage: backfill --from <block> --to <block> [options]

Pushes an explicit, inclusive block range through the indexer's registered
processors, then exits. It moves no cursor and detects no reorgs, so the range
has to sit at or below the safe head (the chain head less FINALITY_DEPTH).

Options:
  --from <block>        First block, inclusive. Required.
  --to <block>          Last block, inclusive. Required.
  --processors <names>  Comma-separated, and repeatable. Defaults to every
                        processor registered in the indexer.
  --range-size <n>      Blocks per dispatch. Defaults to INDEXER_MAX_RANGE_SIZE.
  --max-attempts <n>    Attempts at one range before giving up. Defaults to 5.
  --dry-run             Validate everything and report the plan, dispatching
                        nothing.
  -h, --help            This message.

Chain, providers and log level come from the environment, exactly as the
indexer itself reads them. See apps/indexer/.env.example.
`;

export type ParsedArgs =
  | { readonly kind: 'run'; readonly request: BackfillRequest }
  | { readonly kind: 'help' }
  | { readonly kind: 'invalid'; readonly reason: string };

/**
 * Values are validated with Zod rather than by hand, so an argument error reads
 * like the environment errors do and is formatted by the same `prettifyError`.
 */
const schema = z
  .object({
    from: z.coerce.number().int().min(0),
    to: z.coerce.number().int().min(0),
    processors: z.array(z.string().min(1)).optional(),
    rangeSize: z.coerce.number().int().min(1).max(100_000).optional(),
    maxAttempts: z.coerce.number().int().min(1).max(100).optional(),
    dryRun: z.boolean(),
  })
  .refine((value) => value.from <= value.to, {
    error: 'the range runs backwards: --from must not exceed --to',
    path: ['from'],
  });

/** Accepts both `--processors a,b` and `--processors a --processors b`. */
function names(values: string[] | undefined): string[] | undefined {
  if (values === undefined) return undefined;

  const parsed = values
    .flatMap((value) => value.split(','))
    .map((name) => name.trim())
    .filter((name) => name.length > 0);

  return parsed.length > 0 ? parsed : undefined;
}

/**
 * `argv` without the executable and script — `process.argv.slice(2)`.
 *
 * Pure, so the whole surface is testable without booting anything. The one
 * check it cannot make is whether the range sits below the safe head: that
 * needs the chain, so it belongs to the runner.
 */
export function parseBackfillArgs(argv: readonly string[]): ParsedArgs {
  // `pnpm run <script> -- --flag` forwards the separator verbatim, where npm
  // swallows it. Nothing here takes a positional, so a leading `--` can only be
  // that passthrough — and rejecting it would fail the form most people reach
  // for first.
  const args = argv[0] === '--' ? argv.slice(1) : argv;

  let values;
  try {
    ({ values } = parseArgs({
      args: [...args],
      options: {
        from: { type: 'string' },
        to: { type: 'string' },
        processors: { type: 'string', multiple: true },
        'range-size': { type: 'string' },
        'max-attempts': { type: 'string' },
        'dry-run': { type: 'boolean', default: false },
        help: { type: 'boolean', short: 'h', default: false },
      },
      strict: true,
      allowPositionals: false,
    }));
  } catch (error) {
    // `strict` rejects an unknown or malformed option, naming it.
    return { kind: 'invalid', reason: error instanceof Error ? error.message : String(error) };
  }

  if (values.help) return { kind: 'help' };

  // Checked before Zod sees them: coercing `undefined` yields `NaN`, and
  // "expected number, received NaN" is a poor way to say "you left --from out".
  const missing = (['from', 'to'] as const).filter((option) => values[option] === undefined);
  if (missing.length > 0) {
    return {
      kind: 'invalid',
      reason: `missing required option(s): ${missing.map((option) => `--${option}`).join(', ')}`,
    };
  }

  const parsed = schema.safeParse({
    from: values.from,
    to: values.to,
    processors: names(values.processors),
    rangeSize: values['range-size'],
    maxAttempts: values['max-attempts'],
    dryRun: values['dry-run'],
  });

  if (!parsed.success) {
    return { kind: 'invalid', reason: z.prettifyError(parsed.error) };
  }

  return { kind: 'run', request: parsed.data };
}
