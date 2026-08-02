import { describe, expect, it } from 'vitest';

import { InvalidCursorError, decodeCursor, encodeCursor } from './position-cursor';

const CURSOR = {
  user: '0x82d16ff1c724ab72f218a3f7f6dd3e5385ee87e8',
  spoke: '0x94e7a5dcbe816e498b89ab752661904e2f56c485',
  reserveId: '13',
};

describe('position cursor', () => {
  it('round-trips the sorting key it was built from', () => {
    expect(decodeCursor(encodeCursor(CURSOR))).toEqual(CURSOR);
  });

  it('survives base64url, so it is safe in a query string unescaped', () => {
    expect(encodeCursor(CURSOR)).toMatch(/^[\w-]+$/);
  });

  it.each([
    ['truncated', Buffer.from('0xabc|0xdef').toString('base64url')],
    [
      'a checksummed user',
      encodeCursor({ ...CURSOR, user: '0x82D16fF1C724ab72F218A3f7f6DD3E5385ee87E8' }),
    ],
    ['a non-numeric reserve id', encodeCursor({ ...CURSOR, reserveId: '13; DROP' })],
    ['not base64 at all', 'oh hello'],
  ])('rejects %s', (_case, encoded) => {
    // Named, and thrown here rather than surfacing as a ClickHouse parse error
    // from inside a query — or worse, silently paging from somewhere the caller
    // did not ask for. The parts are bound as parameters, so this is about a
    // legible failure rather than injection.
    expect(() => decodeCursor(encoded)).toThrow(InvalidCursorError);
  });
});
