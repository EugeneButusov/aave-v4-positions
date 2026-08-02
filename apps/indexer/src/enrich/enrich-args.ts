import { parseArgs } from 'node:util';

export const USAGE = `Usage: enrich:tokens [--force] [--token <address>]

Reads what the Hub's listed ERC-20s call themselves and stores it.

**Not the mechanism.** The indexer fills this in on its own — a new listing
is enriched in the dispatch that ingests it, and a sweep behind that covers
a cold start. This command exists for the two things the automatic path
deliberately will not do: re-read a token it already has, and read one you
name.

Options:
  --force            Re-read tokens that already have a row. The sweep only
                     closes absent rows, never revisits wrong ones.
  --token <address>  Just this one. Implies --force.
  -h, --help         This message.

Exit codes: 0 every gap closed, 1 a gap remains or decimals disagreed,
2 bad arguments, 130 interrupted.

Chain, providers and both databases come from the environment, exactly as
the indexer reads them.
`;

export interface EnrichRequest {
  readonly force: boolean;
  readonly token: string | null;
}

export type ParsedEnrichArgs =
  | { readonly kind: 'run'; readonly request: EnrichRequest }
  | { readonly kind: 'help' }
  | { readonly kind: 'invalid'; readonly reason: string };

const ADDRESS = /^0x[0-9a-fA-F]{40}$/;

/** Pure, so the whole surface is testable without booting anything. */
export function parseEnrichArgs(argv: readonly string[]): ParsedEnrichArgs {
  let values;
  try {
    ({ values } = parseArgs({
      args: [...argv],
      options: {
        force: { type: 'boolean', default: false },
        token: { type: 'string' },
        help: { type: 'boolean', short: 'h', default: false },
      },
      strict: true,
    }));
  } catch (error) {
    return { kind: 'invalid', reason: error instanceof Error ? error.message : String(error) };
  }

  if (values.help ?? false) return { kind: 'help' };

  const token = values.token ?? null;
  if (token !== null && !ADDRESS.test(token)) {
    return { kind: 'invalid', reason: `--token must be a 20-byte hex address, got "${token}"` };
  }

  return {
    kind: 'run',
    // Naming a token means you want it read, and it almost certainly already
    // has a row — so the flag it would need is implied rather than demanded.
    request: {
      force: (values.force ?? false) || token !== null,
      token: token?.toLowerCase() ?? null,
    },
  };
}
