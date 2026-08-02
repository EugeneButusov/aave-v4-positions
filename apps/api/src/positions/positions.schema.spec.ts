import { describe, expect, it } from 'vitest';

import {
  DEFAULT_PAGE_SIZE,
  MAX_PAGE_SIZE,
  positionParamsSchema,
  positionQuerySchema,
} from './positions.schema';

/** Checksummed, as viem and every block explorer hand them back. */
const ALICE = '0x82D16fF1C724ab72F218A3f7f6DD3E5385ee87E8';
const SPOKE = '0x94e7A5dCbE816e498b89aB752661904E2F56c485';

describe('positionParamsSchema', () => {
  it('coerces the chain id out of its path segment', () => {
    expect(positionParamsSchema.parse({ chainId: '1', user: ALICE })).toMatchObject({ chainId: 1 });
  });

  it.each([
    ['not a number', 'abc'],
    ['empty', ''],
    ['negative', '-1'],
    ['fractional', '1.5'],
  ])('refuses a chain id that is %s', (_case, chainId) => {
    // Coercion failing open is the danger: `Number('abc')` is NaN and `Number('')`
    // is 0, so without the bounds `/chains/abc/...` would be a query for a chain
    // that does not exist rather than an error.
    expect(positionParamsSchema.safeParse({ chainId, user: ALICE }).success).toBe(false);
  });

  it('lower-cases the wallet, so a checksummed address off an explorer matches', () => {
    expect(positionParamsSchema.parse({ chainId: '1', user: ALICE })).toMatchObject({
      user: ALICE.toLowerCase(),
    });
  });

  it.each([
    ['too short', '0x82d16ff1'],
    ['too long', `${ALICE}00`],
    ['not hex', `0x${'z'.repeat(40)}`],
    ['no 0x prefix', ALICE.slice(2)],
  ])('refuses a wallet that is %s', (_case, user) => {
    expect(positionParamsSchema.safeParse({ chainId: '1', user }).success).toBe(false);
  });
});

describe('positionQuerySchema', () => {
  it('needs nothing at all', () => {
    // The Spoke is genuinely optional: a caller asking what a wallet holds
    // should not have to know which Spokes exist.
    expect(positionQuerySchema.parse({})).toEqual({ limit: DEFAULT_PAGE_SIZE });
  });

  it('lower-cases the Spoke when one is given', () => {
    expect(positionQuerySchema.parse({ spoke: SPOKE })).toMatchObject({
      spoke: SPOKE.toLowerCase(),
    });
  });

  it('coerces and bounds the page size', () => {
    expect(positionQuerySchema.parse({ limit: '10' })).toMatchObject({ limit: 10 });
    expect(positionQuerySchema.parse({ limit: String(MAX_PAGE_SIZE) })).toMatchObject({
      limit: MAX_PAGE_SIZE,
    });
    expect(positionQuerySchema.safeParse({ limit: '0' }).success).toBe(false);
    expect(positionQuerySchema.safeParse({ limit: String(MAX_PAGE_SIZE + 1) }).success).toBe(false);
  });

  it('refuses a parameter it does not recognise', () => {
    // `?limt=200` would otherwise strip silently and serve the default, so the
    // caller reads a page size they never asked for and believes they set one.
    const result = positionQuerySchema.safeParse({ limt: '200' });

    expect(result.success).toBe(false);
    expect(result.error?.issues[0]?.code).toBe('unrecognized_keys');
  });

  it('takes an asOf, and refuses one that is obviously milliseconds', () => {
    expect(positionQuerySchema.parse({ asOf: '1785000000' })).toMatchObject({
      asOf: 1_785_000_000,
    });

    // The likeliest wrong value by far. Unbounded, it would value every
    // position tens of thousands of years out — which is not an error, just a
    // page of numbers nobody can tell is wrong.
    expect(positionQuerySchema.safeParse({ asOf: '1785000000000' }).success).toBe(false);
    expect(positionQuerySchema.safeParse({ asOf: '1' }).success).toBe(false);
  });

  it('refuses an empty cursor rather than treating it as absent', () => {
    // `?cursor=` is a caller who lost their cursor, not one starting over.
    expect(positionQuerySchema.safeParse({ cursor: '' }).success).toBe(false);
  });
});
