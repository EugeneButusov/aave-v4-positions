import { createHmac, timingSafeEqual } from 'node:crypto';

import { BadRequestException, Injectable } from '@nestjs/common';
import type { PositionKey } from '@aave-positions/positions';

/**
 * The listing a page belongs to: one wallet, on one chain, and either one Spoke
 * or all of them.
 *
 * **`spoke` is the filter that was applied, not the Spoke a row came from.**
 * `null` means the listing spanned every Spoke. The distinction is the whole
 * point: an all-Spokes cursor is a well-formed resume point inside a
 * single-Spoke listing, so if the two scopes signed identically a caller could
 * carry one across and silently skip every reserve below it.
 *
 * Not carried inside the cursor — mixed into its signature instead, so a tag
 * only verifies against the listing it was issued for.
 *
 * **Precondition: `user` and `spoke` are lower-case `0x` addresses**, which
 * `positionParamsSchema` and `positionQuerySchema` guarantee by parsing them at
 * the edge. It is not re-checked here, and it is load-bearing rather than
 * cosmetic — see {@link SEP}.
 */
export interface CursorScope {
  readonly chainId: number;
  readonly user: string;
  readonly spoke: string | null;
}

/** Lower-case, because that is the only form the fold stores. */
const ADDRESS = /^0x[0-9a-f]{40}$/;
const DIGITS = /^\d+$/;

/**
 * The one separator, between every field of both the scope and the payload.
 *
 * One rather than a hierarchy of them, because what makes a concatenation
 * unambiguous is not picking a rare character — it is that the separator cannot
 * appear in the values it separates. Every field here is an address, a decimal
 * id or {@link ALL_SPOKES}: none can contain this, so `a|b|c` has exactly one
 * reading. Were that not true, a tag issued for one listing would verify
 * against another with the boundary moved, and no choice of character would be
 * safe — only less obviously unsafe.
 */
const SEP = '|';

/** Not a valid address, so it cannot collide with a Spoke that is genuinely filtered on. */
const ALL_SPOKES = '*';

/** 128 bits. Full SHA-256 would triple the cursor to no benefit. */
const TAG_BYTES = 16;

/** A key shorter than this is guessable, and a guessable key is not a signature. */
const MIN_SECRET_LENGTH = 32;

/**
 * The cursor as this API publishes it.
 *
 * **Signing lives here, not in `@aave-positions/positions`.** That package deals
 * in {@link PositionKey} — keyset paging is how the database resumes a scan,
 * and it is the same page key whether a CLI reconciliation asks for it or an
 * HTTP request does. Making that key opaque and unforgeable is a property of
 * *publishing* it: it exists because this service hands the key to someone it
 * does not trust and takes it back again. Nothing inside the fold ever holds a
 * signed cursor, and the secret that signs one is this service's configuration.
 *
 * The resume point is `(spoke, reserveId)`, because the scope pins everything
 * above it: within one `(chain, user)` the table's sorting key has only those
 * two left. When the listing is already narrowed to one Spoke the first half is
 * constant, but it is still signed — that is what stops the two listings sharing
 * a cursor.
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
 * stored in the cursor**. A tag over `(chainId, user, spoke-filter)` plus the key
 * only verifies when the caller presents the same three, so switching listings
 * fails the same check as tampering — and the cursor stays one field long.
 *
 * HMAC rather than a JWT: a JWT would add a header, algorithm negotiation and
 * the `alg: none` footgun to protect two short strings.
 *
 * **A cursor that does not verify is a 400.** It is the caller's input being
 * wrong, not this service failing, and it is raised at the point of detection
 * rather than translated afterwards — so a genuine fault in here still surfaces
 * as the 500 it is, with no catch-all to swallow it.
 *
 * **The key must be identical across replicas.** Each process signs with its own
 * copy, so a per-process secret means a cursor issued by one pod is rejected by
 * the next — pagination that fails only under load, and only sometimes.
 */
@Injectable()
export class PositionCursors {
  private readonly key: Buffer;

  constructor(secret: string) {
    if (secret.length < MIN_SECRET_LENGTH) {
      throw new Error(
        `cursor secret must be at least ${MIN_SECRET_LENGTH} characters, got ${secret.length}`,
      );
    }
    this.key = Buffer.from(secret, 'utf8');
  }

  encode(scope: CursorScope, key: PositionKey): string {
    const payload = `${key.spoke}${SEP}${key.reserveId}`;
    return `${Buffer.from(payload).toString('base64url')}.${this.tag(scope, payload)}`;
  }

  /**
   * Verifies before it parses — unauthenticated input is not worth decoding.
   *
   * Both halves are bound as query parameters downstream, so the format checks
   * are not an injection defence; they are there so a malformed cursor fails
   * here by name rather than as a parse error from inside a query.
   */
  decode(encoded: string, scope: CursorScope): PositionKey {
    const [body = '', tag = '', ...rest] = encoded.split('.');
    if (rest.length > 0) throw invalid('expected a payload and a tag');

    const payload = Buffer.from(body, 'base64url').toString('utf8');
    const expected = Buffer.from(this.tag(scope, payload));
    const actual = Buffer.from(tag);
    // Length first: timingSafeEqual throws on a mismatch rather than returning
    // false, which would turn a forged cursor into a 500.
    if (actual.length !== expected.length || !timingSafeEqual(actual, expected)) {
      throw invalid('signature does not match this listing');
    }

    const [spoke = '', reserveId = '', ...extra] = payload.split(SEP);
    if (extra.length > 0) throw invalid('expected a Spoke and a reserve id');
    if (!ADDRESS.test(spoke)) throw invalid(`Spoke "${spoke}" is not a lower-case address`);
    if (!DIGITS.test(reserveId)) throw invalid(`reserve id "${reserveId}" is not a number`);

    return { spoke, reserveId };
  }

  /** Every field, joined by the one separator none of them can contain. */
  private tag(scope: CursorScope, payload: string): string {
    const fields = [scope.chainId, scope.user, scope.spoke ?? ALL_SPOKES, payload];
    return createHmac('sha256', this.key)
      .update(fields.join(SEP))
      .digest()
      .subarray(0, TAG_BYTES)
      .toString('base64url');
  }
}

/** Named, so every refusal reads the same and none of them is a 500. */
function invalid(reason: string): BadRequestException {
  return new BadRequestException(`invalid page cursor: ${reason}`);
}
