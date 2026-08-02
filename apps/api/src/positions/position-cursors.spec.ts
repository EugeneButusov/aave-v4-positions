import { BadRequestException } from '@nestjs/common';
import type { PositionKey } from '@aave-positions/positions';
import { describe, expect, it } from 'vitest';

import { PositionCursors, type CursorScope } from './position-cursors';

const SECRET = 'a'.repeat(32);
const cursors = new PositionCursors(SECRET);

const ALICE = '0x82d16ff1c724ab72f218a3f7f6dd3e5385ee87e8';
const BOB = '0xb8516f75dcf450b5b455b5114f5a92f6abd37dca';
const SPOKE = '0x94e7a5dcbe816e498b89ab752661904e2f56c485';
const OTHER_SPOKE = '0x973a023a77420ba610f06b3858ad991df6d85a08';

const SCOPE: CursorScope = { chainId: 1, user: ALICE, spoke: SPOKE };
/** The same wallet, listed across every Spoke rather than narrowed to one. */
const ALL_SPOKES: CursorScope = { ...SCOPE, spoke: null };

const KEY: PositionKey = { spoke: SPOKE, reserveId: '13' };

/** Re-signs a resume point with a key this service does not hold. */
function forge(key: PositionKey, secret: string, scope: CursorScope = SCOPE): string {
  return new PositionCursors(secret).encode(scope, key);
}

/** Edits the payload and leaves the tag the caller was given. */
function tamper(encoded: string, payload: string): string {
  const tag = encoded.split('.')[1] ?? '';
  return `${Buffer.from(payload).toString('base64url')}.${tag}`;
}

describe('PositionCursors', () => {
  it('round-trips the resume point it was built from', () => {
    expect(cursors.decode(cursors.encode(SCOPE, KEY), SCOPE)).toEqual(KEY);
  });

  it('carries the Spoke, so an all-Spokes walk knows where it stopped', () => {
    const key: PositionKey = { spoke: OTHER_SPOKE, reserveId: '3' };

    // With `spoke` unpinned it is half of what the sorting key leaves free, so
    // a resume point without it would restart at whichever Spoke sorts first.
    expect(cursors.decode(cursors.encode(ALL_SPOKES, key), ALL_SPOKES)).toEqual(key);
  });

  it('stays URL-safe, so it needs no escaping in a query string', () => {
    expect(cursors.encode(SCOPE, KEY)).toMatch(/^[\w-]+\.[\w-]+$/);
  });

  it('refuses a secret short enough to guess', () => {
    // At construction, so a misconfigured deployment fails to boot rather than
    // serving forgeable cursors.
    expect(() => new PositionCursors('too-short')).toThrow(/at least 32/);
  });

  describe('rejects', () => {
    it('a payload edited under a tag we issued', () => {
      const issued = cursors.encode(SCOPE, KEY);

      // The whole point: without a signature this is a valid resume point, and
      // the caller has silently moved themselves somewhere they were not sent.
      expect(() => cursors.decode(tamper(issued, `${SPOKE}|9999`), SCOPE)).toThrow(
        BadRequestException,
      );
    });

    it('a cursor signed with a different key', () => {
      expect(() => cursors.decode(forge(KEY, 'b'.repeat(32)), SCOPE)).toThrow(BadRequestException);
    });

    it("a cursor issued for one wallet and replayed against another's listing", () => {
      const issued = cursors.encode(SCOPE, KEY);

      // The correctness hole a bare signature would leave open. The key is
      // well-formed and genuinely ours — it just names a resume point in a
      // different listing, so the scope goes into the tag rather than beside it.
      expect(() => cursors.decode(issued, { ...SCOPE, user: BOB })).toThrow(
        /does not match this listing/,
      );
      expect(cursors.decode(issued, SCOPE)).toEqual(KEY);
    });

    it('an all-Spokes cursor replayed on a single-Spoke listing', () => {
      // Both directions, because the sentinel only has to be wrong one way for
      // this to pass by accident. An all-Spokes resume point is *well-formed*
      // inside the narrowed listing — it names a Spoke and a reserve — so
      // nothing downstream would notice it skipping every reserve below it.
      const broad = cursors.encode(ALL_SPOKES, KEY);
      const narrow = cursors.encode(SCOPE, KEY);

      expect(() => cursors.decode(broad, SCOPE)).toThrow(/does not match this listing/);
      expect(() => cursors.decode(narrow, ALL_SPOKES)).toThrow(/does not match this listing/);
    });

    it.each([
      ['another chain', { ...SCOPE, chainId: 8453 }],
      ['another Spoke', { ...SCOPE, spoke: OTHER_SPOKE }],
    ])('a cursor from %s', (_case, scope) => {
      expect(() => cursors.decode(cursors.encode(scope, KEY), SCOPE)).toThrow(BadRequestException);
    });

    it.each([
      ['no tag at all', 'aGVsbG8'],
      ['more segments than we issue', 'a.b.c'],
      ['not base64', 'oh hello.and again'],
      ['an empty string', ''],
    ])('%s', (_case, encoded) => {
      expect(() => cursors.decode(encoded, SCOPE)).toThrow(BadRequestException);
    });

    it.each([
      ['a non-numeric reserve id', { spoke: SPOKE, reserveId: '13; DROP' }],
      ['an empty reserve id', { spoke: SPOKE, reserveId: '' }],
      ['a checksummed Spoke', { spoke: SPOKE.toUpperCase(), reserveId: '13' }],
      ['no Spoke at all', { spoke: '', reserveId: '13' }],
    ])('%s, even when correctly signed', (_case, key) => {
      // Reachable only through a bug on our side, since a caller cannot produce
      // a valid tag. It fails by name here rather than as a parse error from
      // inside a query.
      expect(() => cursors.decode(forge(key, SECRET), SCOPE)).toThrow(BadRequestException);
    });
  });

  describe('as an HTTP failure', () => {
    it('is a 400, not a 500', () => {
      // A forged cursor is the caller's input being wrong. A 500 here would be
      // a page an operator gets woken for, over a query parameter.
      const thrown = capture(() => cursors.decode('not-one-of-ours', SCOPE));

      expect(thrown?.getStatus()).toBe(400);
    });

    it('says which listing refused it, rather than only "Bad Request"', () => {
      const thrown = capture(() => cursors.decode(cursors.encode(SCOPE, KEY), ALL_SPOKES));

      expect(thrown?.getResponse()).toMatchObject({
        statusCode: 400,
        message: expect.stringContaining('does not match this listing'),
      });
    });
  });
});

/** Returns what was thrown, so the assertion is not inside a catch block. */
function capture(run: () => unknown): BadRequestException | undefined {
  try {
    run();
    return undefined;
  } catch (error) {
    return error as BadRequestException;
  }
}
