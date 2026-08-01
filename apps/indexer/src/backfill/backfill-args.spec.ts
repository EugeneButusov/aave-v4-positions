import { describe, expect, it } from 'vitest';

import { parseBackfillArgs, type ParsedArgs } from './backfill-args';

/** The two options with no default; everything else is optional. */
const REQUIRED = ['--from', '100', '--to', '200'];

function parse(...extra: string[]): ParsedArgs {
  return parseBackfillArgs([...REQUIRED, ...extra]);
}

describe('parseBackfillArgs', () => {
  it('reads a range, which arrives as strings', () => {
    expect(parse()).toEqual({
      kind: 'run',
      request: { from: 100, to: 200, dryRun: false },
    });
  });

  it('rejects a missing range rather than inventing one', () => {
    expect(parseBackfillArgs([])).toMatchObject({ kind: 'invalid' });
    expect(parseBackfillArgs(['--from', '100'])).toMatchObject({ kind: 'invalid' });

    const result = parseBackfillArgs(['--to', '200']);
    expect(result).toHaveProperty('reason', expect.stringContaining('--from'));
  });

  it('rejects a range that runs backwards', () => {
    const result = parseBackfillArgs(['--from', '200', '--to', '100']);

    expect(result).toMatchObject({ kind: 'invalid' });
    expect(result).toHaveProperty('reason', expect.stringContaining('backwards'));
  });

  it('accepts a single-block range', () => {
    expect(parseBackfillArgs(['--from', '100', '--to', '100'])).toMatchObject({
      kind: 'run',
      request: { from: 100, to: 100 },
    });
  });

  it('rejects a block number that is negative or not a whole number', () => {
    expect(parseBackfillArgs(['--from', '-1', '--to', '200'])).toMatchObject({ kind: 'invalid' });
    expect(parseBackfillArgs(['--from', '1.5', '--to', '200'])).toMatchObject({ kind: 'invalid' });
    expect(parseBackfillArgs(['--from', 'abc', '--to', '200'])).toMatchObject({ kind: 'invalid' });
  });

  it('splits a comma-separated processor list', () => {
    expect(parse('--processors', 'spoke,hub')).toMatchObject({
      request: { processors: ['spoke', 'hub'] },
    });
  });

  it('accumulates a repeated processor option', () => {
    expect(parse('--processors', 'spoke', '--processors', 'hub')).toMatchObject({
      request: { processors: ['spoke', 'hub'] },
    });
  });

  it('trims the names and drops the empty ones', () => {
    expect(parse('--processors', ' spoke , , hub ')).toMatchObject({
      request: { processors: ['spoke', 'hub'] },
    });
  });

  it('leaves the processor list unset when none is given, meaning all of them', () => {
    expect(parse()).toHaveProperty('request.processors', undefined);
  });

  it('reads the tuning options', () => {
    expect(parse('--range-size', '250', '--max-attempts', '2')).toMatchObject({
      request: { rangeSize: 250, maxAttempts: 2 },
    });
  });

  it('rejects a range size or attempt count below one', () => {
    expect(parse('--range-size', '0')).toMatchObject({ kind: 'invalid' });
    expect(parse('--max-attempts', '0')).toMatchObject({ kind: 'invalid' });
  });

  it('reads the dry-run flag', () => {
    expect(parse('--dry-run')).toMatchObject({ request: { dryRun: true } });
    expect(parse()).toMatchObject({ request: { dryRun: false } });
  });

  it('answers help for either spelling, without needing a valid range', () => {
    expect(parseBackfillArgs(['--help'])).toEqual({ kind: 'help' });
    expect(parseBackfillArgs(['-h'])).toEqual({ kind: 'help' });
  });

  it('rejects an unknown option, naming it', () => {
    const result = parse('--processor', 'spoke');

    expect(result).toMatchObject({ kind: 'invalid' });
    expect(result).toHaveProperty('reason', expect.stringContaining('--processor'));
  });

  it('rejects a stray positional rather than ignoring it', () => {
    expect(parse('249')).toMatchObject({ kind: 'invalid' });
  });

  it('tolerates the separator pnpm forwards verbatim', () => {
    expect(parseBackfillArgs(['--', '--from', '100', '--to', '200'])).toMatchObject({
      kind: 'run',
      request: { from: 100, to: 200 },
    });
    expect(parseBackfillArgs(['--', '--help'])).toEqual({ kind: 'help' });
  });
});
