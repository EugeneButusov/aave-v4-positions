import { describe, expect, it } from 'vitest';

import { parseEnrichArgs } from './enrich-args';

const USDC = '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48';

describe('parseEnrichArgs', () => {
  it('defaults to closing gaps only', () => {
    expect(parseEnrichArgs([])).toEqual({
      kind: 'run',
      request: { force: false, token: null },
    });
  });

  it('takes --force', () => {
    expect(parseEnrichArgs(['--force'])).toMatchObject({ request: { force: true } });
  });

  it('implies --force when a token is named', () => {
    // Naming one almost always means re-reading one that already has a row.
    // Demanding both flags would be ceremony over the only reason to name it.
    expect(parseEnrichArgs(['--token', USDC])).toEqual({
      kind: 'run',
      request: { force: true, token: USDC },
    });
  });

  it('lower-cases the address it is given', () => {
    // The fold stores what the log carried; a checksummed address would be
    // written as a second, unjoinable row.
    const parsed = parseEnrichArgs(['--token', '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48']);
    expect(parsed).toMatchObject({ request: { token: USDC } });
  });

  it('refuses something that is not an address', () => {
    expect(parseEnrichArgs(['--token', 'USDC'])).toMatchObject({ kind: 'invalid' });
  });

  it('refuses an unknown flag rather than ignoring it', () => {
    // `--froce` silently doing nothing is how an operator concludes the command
    // is broken.
    expect(parseEnrichArgs(['--froce'])).toMatchObject({ kind: 'invalid' });
  });

  it('answers help', () => {
    expect(parseEnrichArgs(['--help'])).toEqual({ kind: 'help' });
    expect(parseEnrichArgs(['-h'])).toEqual({ kind: 'help' });
  });
});
