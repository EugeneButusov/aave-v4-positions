import { describe, expect, it } from 'vitest';

import { parsePriceArgs } from './price-args';

describe('parsePriceArgs', () => {
  it('defaults to reading and storing', () => {
    expect(parsePriceArgs([])).toEqual({ kind: 'run', request: { dryRun: false } });
  });

  it('takes --dry-run', () => {
    expect(parsePriceArgs(['--dry-run'])).toEqual({ kind: 'run', request: { dryRun: true } });
  });

  it('refuses an unknown flag rather than ignoring it', () => {
    // `--dryrun` silently doing nothing is how an operator concludes the
    // command wrote when it did not.
    expect(parsePriceArgs(['--dryrun'])).toMatchObject({ kind: 'invalid' });
  });

  it('refuses a value where a flag was expected', () => {
    expect(parsePriceArgs(['--dry-run=maybe'])).toMatchObject({ kind: 'invalid' });
  });

  it('answers help', () => {
    expect(parsePriceArgs(['--help'])).toEqual({ kind: 'help' });
    expect(parsePriceArgs(['-h'])).toEqual({ kind: 'help' });
  });
});
