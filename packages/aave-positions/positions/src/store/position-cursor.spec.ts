import { describe, expect, it } from 'vitest';

import { InvalidCursorError, PositionCursorCodec, type CursorScope } from './position-cursor';

const SECRET = 'a'.repeat(32);
const codec = new PositionCursorCodec(SECRET);

const ALICE = '0x82d16ff1c724ab72f218a3f7f6dd3e5385ee87e8';
const BOB = '0xb8516f75dcf450b5b455b5114f5a92f6abd37dca';
const SPOKE = '0x94e7a5dcbe816e498b89ab752661904e2f56c485';

const SCOPE: CursorScope = { chainId: 1, user: ALICE, spoke: SPOKE };
const RESERVE = '13';

/** Re-signs a resume point with a key the store does not hold. */
function forge(reserveId: string, secret: string, scope: CursorScope = SCOPE): string {
  return new PositionCursorCodec(secret).encode(scope, reserveId);
}

/** Edits the payload and leaves the tag the caller was given. */
function tamper(encoded: string, reserveId: string): string {
  const tag = encoded.split('.')[1] ?? '';
  return `${Buffer.from(reserveId).toString('base64url')}.${tag}`;
}

describe('PositionCursorCodec', () => {
  it('round-trips the resume point it was built from', () => {
    expect(codec.decode(codec.encode(SCOPE, RESERVE), SCOPE)).toBe(RESERVE);
  });

  it('stays URL-safe, so it needs no escaping in a query string', () => {
    expect(codec.encode(SCOPE, RESERVE)).toMatch(/^[\w-]+\.[\w-]+$/);
  });

  it('refuses a secret short enough to guess', () => {
    expect(() => new PositionCursorCodec('too-short')).toThrow(/at least 32/);
  });

  describe('rejects', () => {
    it('a payload edited under a tag we issued', () => {
      const issued = codec.encode(SCOPE, RESERVE);

      // The whole point: without a signature this is a valid resume point, and
      // the caller has silently moved themselves somewhere they were not sent.
      expect(() => codec.decode(tamper(issued, '9999'), SCOPE)).toThrow(InvalidCursorError);
    });

    it('a cursor signed with a different key', () => {
      expect(() => codec.decode(forge(RESERVE, 'b'.repeat(32)), SCOPE)).toThrow(InvalidCursorError);
    });

    it("a cursor issued for one wallet and replayed against another's listing", () => {
      const issued = codec.encode(SCOPE, RESERVE);

      // The correctness hole a bare signature would leave open. The reserve id
      // is well-formed and genuinely ours — it just names a resume point in a
      // different listing, so the scope goes into the tag rather than beside it.
      expect(() => codec.decode(issued, { ...SCOPE, user: BOB })).toThrow(
        /does not match this listing/,
      );
      expect(codec.decode(issued, SCOPE)).toBe(RESERVE);
    });

    it.each([
      ['another chain', { ...SCOPE, chainId: 8453 }],
      ['another Spoke', { ...SCOPE, spoke: BOB }],
    ])('a cursor from %s', (_case, scope) => {
      expect(() => codec.decode(codec.encode(scope, RESERVE), SCOPE)).toThrow(InvalidCursorError);
    });

    it.each([
      ['no tag at all', 'aGVsbG8'],
      ['more segments than we issue', 'a.b.c'],
      ['not base64', 'oh hello.and again'],
      ['an empty string', ''],
    ])('%s', (_case, encoded) => {
      expect(() => codec.decode(encoded, SCOPE)).toThrow(InvalidCursorError);
    });

    it.each([
      ['a non-numeric reserve id', '13; DROP'],
      ['an empty reserve id', ''],
    ])('%s, even when correctly signed', (_case, reserveId) => {
      // Reachable only through a bug on our side, since a caller cannot produce
      // a valid tag. It fails by name here rather than as a parse error from
      // inside a query.
      expect(() => codec.decode(forge(reserveId, SECRET), SCOPE)).toThrow(InvalidCursorError);
    });
  });
});
