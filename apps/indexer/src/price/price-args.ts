import { parseArgs } from 'node:util';

export const USAGE = `Usage: price:reserves [--dry-run]

Reads what the Spoke's oracle prices each of its reserves at, and stores it.

**Not the mechanism.** The indexer refreshes these on its own, off the indexing
loop, every RESERVE_PRICE_REFRESH_MS. This command exists for the two things
that path deliberately will not do: read when you ask rather than when the timer
says, and report every price it got rather than only logging a count.

It is also how a price is proven to be reachable at all — an oracle that reverts
shows up here as a named reserve rather than as a USD value that is quietly null
on the endpoint.

Options:
  --dry-run   Read and report, write nothing.
  -h, --help  This message.

Exit codes: 0 every reserve priced, 1 one or more could not be, 2 bad arguments,
130 interrupted.

Chain, providers, the Spoke, its oracle and both databases come from the
environment, exactly as the indexer reads them.
`;

export interface PriceRequest {
  readonly dryRun: boolean;
}

export type ParsedPriceArgs =
  | { readonly kind: 'run'; readonly request: PriceRequest }
  | { readonly kind: 'help' }
  | { readonly kind: 'invalid'; readonly reason: string };

/** Pure, so the whole surface is testable without booting anything. */
export function parsePriceArgs(argv: readonly string[]): ParsedPriceArgs {
  let values;
  try {
    ({ values } = parseArgs({
      args: [...argv],
      options: {
        'dry-run': { type: 'boolean', default: false },
        help: { type: 'boolean', short: 'h', default: false },
      },
      strict: true,
    }));
  } catch (error) {
    return { kind: 'invalid', reason: error instanceof Error ? error.message : String(error) };
  }

  if (values.help ?? false) return { kind: 'help' };

  return { kind: 'run', request: { dryRun: values['dry-run'] ?? false } };
}
