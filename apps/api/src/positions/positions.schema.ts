import { z } from 'zod';

/**
 * A wallet holds at most one position per reserve, and the Main Spoke has
 * fourteen. Paging is a contract formality at this size rather than a load
 * concern, so these are constants: a knob nobody turns is configuration that
 * only ever goes wrong.
 */
export const DEFAULT_PAGE_SIZE = 50;
export const MAX_PAGE_SIZE = 200;

/**
 * 2026-03-23T14:45:59Z — the Main Spoke's first log, and the earliest instant
 * any position can be valued at.
 */
export const GENESIS_UNIX_SECONDS = 1_774_277_159;

/**
 * 2100-01-01, and it is a **units check rather than a policy** on how far ahead
 * a caller may value.
 *
 * `asOf` in milliseconds is the wrong value by far the most likely to be sent,
 * and it is past the floor above — so without a ceiling it would be accepted,
 * extrapolate the interest index tens of thousands of years, and return a page
 * of enormous numbers with nothing to say they are wrong. Three orders of
 * magnitude below a millisecond timestamp and seventy years above any real one.
 */
export const MAX_ASOF_UNIX_SECONDS = 4_102_444_800;

const address = z
  .string()
  .regex(/^0x[0-9a-fA-F]{40}$/, 'must be a 20-byte hex address')
  // The caller reads a checksummed address off a block explorer; the fold
  // stores it lower-cased. The store lower-cases too, so for matching rows this
  // is belt and braces — but it also makes the value echoed back in the
  // response match what was matched.
  //
  // It is also the **only** place the cursor codec's precondition is enforced.
  // A scope field goes into the signed string beside a separator, so it may not
  // contain one; the codec documents that and does not re-check it, on the
  // principle that a value is parsed once, where it enters. This regex is that
  // place, which is why it is anchored and lower-case rather than lenient.
  .transform((value) => value.toLowerCase());

export const positionParamsSchema = z.object({
  chainId: z.coerce.number().int().positive(),
  user: address,
});

export const positionQuerySchema = z
  .object({
    /** Omitted lists every Spoke this wallet has touched. */
    spoke: address.optional(),
    limit: z.coerce.number().int().min(1).max(MAX_PAGE_SIZE).default(DEFAULT_PAGE_SIZE),
    cursor: z.string().min(1).optional(),
    /**
     * Unix seconds to value the page at. Defaults to now.
     *
     * Amounts accrue every second and the chain emits nothing while they do, so
     * "now" is a choice rather than an absence of one. Naming it explicitly is
     * what makes a response reproducible — ask twice with the same `asOf` and
     * the numbers match — and it is what lets a reconciliation pin both sides
     * to one block.
     *
     * Bounded at both ends, and the bounds are a units check: below the Main
     * Spoke's genesis nothing exists to value, and above 2100 the caller sent
     * milliseconds. Either way the page would come back full of numbers with
     * nothing to say they are wrong.
     */
    asOf: z.coerce.number().int().min(GENESIS_UNIX_SECONDS).max(MAX_ASOF_UNIX_SECONDS).optional(),
  })
  // Strict, so `?limt=200` is a 400 rather than silently serving 50. The cost of
  // being lenient is a caller who believes a parameter took effect, and who
  // finds out by reading a page size they never asked for.
  .strict();

export type PositionParams = z.output<typeof positionParamsSchema>;
export type PositionQueryParams = z.output<typeof positionQuerySchema>;
