/** A cursor the caller handed back that this store did not issue. */
export class InvalidCursorError extends Error {
  constructor(reason: string) {
    super(`invalid page cursor: ${reason}`);
    this.name = 'InvalidCursorError';
  }
}

/**
 * Where the previous page stopped: the sorting key of its last row.
 *
 * Keyset rather than an offset. `LIMIT n OFFSET m` would re-run the whole
 * aggregation to discard `m` rows, and it shifts under concurrent writes — a
 * position crossing a page boundary while the indexer advances is either
 * returned twice or skipped. A key comparison has neither problem, and because
 * this tuple *is* the table's sorting key after `chain_id`, resuming is a seek.
 */
export interface PositionCursor {
  readonly user: string;
  readonly spoke: string;
  readonly reserveId: string;
}

const ADDRESS = /^0x[0-9a-f]{40}$/;
const DIGITS = /^\d+$/;

export function encodeCursor(cursor: PositionCursor): string {
  return Buffer.from(`${cursor.user}|${cursor.spoke}|${cursor.reserveId}`).toString('base64url');
}

/**
 * Opaque to the caller, and validated rather than trusted.
 *
 * The parts are bound as query parameters, so nothing here is an injection
 * seam — but an unparseable reserve id would surface as a ClickHouse parse
 * error from deep in a query, and a truncated cursor would silently page from
 * somewhere the caller did not ask for. Both should be one named failure.
 */
export function decodeCursor(encoded: string): PositionCursor {
  const parts = Buffer.from(encoded, 'base64url').toString('utf8').split('|');
  if (parts.length !== 3) throw new InvalidCursorError('expected three parts');

  const [user = '', spoke = '', reserveId = ''] = parts;
  if (!ADDRESS.test(user))
    throw new InvalidCursorError(`user "${user}" is not a lower-case address`);
  if (!ADDRESS.test(spoke))
    throw new InvalidCursorError(`spoke "${spoke}" is not a lower-case address`);
  if (!DIGITS.test(reserveId))
    throw new InvalidCursorError(`reserve id "${reserveId}" is not a number`);

  return { user, spoke, reserveId };
}
