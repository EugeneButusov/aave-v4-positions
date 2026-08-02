import { createHmac, timingSafeEqual } from 'node:crypto';

/** A cursor the caller handed back that this store did not issue. */
export class InvalidCursorError extends Error {
  constructor(reason: string) {
    super(`invalid page cursor: ${reason}`);
    this.name = 'InvalidCursorError';
  }
}

/**
 * The listing a page belongs to: one wallet, on one Spoke, on one chain.
 *
 * Not carried inside the cursor — mixed into its signature instead, so a tag
 * only verifies against the listing it was issued for. See
 * {@link PositionCursorCodec}.
 */
export interface CursorScope {
  readonly chainId: number;
  readonly user: string;
  readonly spoke: string;
}

const DIGITS = /^\d+$/;

/** 128 bits. Full SHA-256 would triple the cursor to no benefit. */
const TAG_BYTES = 16;

/** A key shorter than this is guessable, and a guessable key is not a signature. */
const MIN_SECRET_LENGTH = 32;

/**
 * Signs pagination cursors, and refuses ones it did not sign.
 *
 * The resume point is a single reserve id, because the scope pins everything
 * above it: within one `(chain, user, spoke)` the table's sorting key has only
 * `reserve_id` left, so that is the whole of "where the last page stopped".
 *
 * **What signing defends against.** Position data is public on chain, so this is
 * neither confidentiality nor access control — a caller can already ask for any
 * wallet. It buys two other things. The cursor becomes a genuinely opaque
 * contract, so its encoding can change without breaking anyone who hand-rolled
 * one. And it stops a cursor being carried between listings: unsigned, a resume
 * point from one wallet is a well-formed resume point in another's, silently
 * skipping every reserve below it.
 *
 * That second one is why the scope is **mixed into the signature rather than
 * stored in the cursor**. A tag over `(chainId, user, spoke)` plus the reserve id
 * only verifies when the caller presents the same three, so switching listings
 * fails the same check as tampering — and the cursor stays one field long.
 *
 * HMAC rather than a JWT: a JWT would add a header, algorithm negotiation and
 * the `alg: none` footgun to protect a single integer.
 *
 * **The key must be identical across replicas.** Each process signs with its own
 * copy, so a per-process secret means a cursor issued by one pod is rejected by
 * the next — pagination that fails only under load, and only sometimes.
 */
export class PositionCursorCodec {
  private readonly key: Buffer;

  constructor(secret: string) {
    if (secret.length < MIN_SECRET_LENGTH) {
      throw new Error(
        `cursor secret must be at least ${MIN_SECRET_LENGTH} characters, got ${secret.length}`,
      );
    }
    this.key = Buffer.from(secret, 'utf8');
  }

  encode(scope: CursorScope, reserveId: string): string {
    return `${Buffer.from(reserveId).toString('base64url')}.${this.tag(scope, reserveId)}`;
  }

  /**
   * Verifies before it parses — unauthenticated input is not worth decoding.
   *
   * The reserve id is bound as a query parameter downstream, so the digit check
   * is not an injection defence; it is there so a malformed cursor fails here by
   * name rather than as a parse error from inside a query.
   */
  decode(encoded: string, scope: CursorScope): string {
    const [body = '', tag = '', ...rest] = encoded.split('.');
    if (rest.length > 0) throw new InvalidCursorError('expected a payload and a tag');

    const reserveId = Buffer.from(body, 'base64url').toString('utf8');
    const expected = Buffer.from(this.tag(scope, reserveId));
    const actual = Buffer.from(tag);
    // Length first: timingSafeEqual throws on a mismatch rather than returning
    // false, which would turn a forged cursor into a 500.
    if (actual.length !== expected.length || !timingSafeEqual(actual, expected)) {
      throw new InvalidCursorError('signature does not match this listing');
    }

    if (!DIGITS.test(reserveId)) {
      throw new InvalidCursorError(`reserve id "${reserveId}" is not a number`);
    }
    return reserveId;
  }

  /** Newline separates scope from payload; neither half can contain one. */
  private tag(scope: CursorScope, reserveId: string): string {
    return createHmac('sha256', this.key)
      .update(`${scope.chainId}|${scope.user}|${scope.spoke}\n${reserveId}`)
      .digest()
      .subarray(0, TAG_BYTES)
      .toString('base64url');
  }
}
