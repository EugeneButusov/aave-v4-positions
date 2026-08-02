import { BadRequestException } from '@nestjs/common';
import { describe, expect, it } from 'vitest';
import { z } from 'zod';

import { ZodValidationPipe } from './zod-validation.pipe';

const schema = z.object({
  count: z.coerce.number().int().min(1),
  name: z.string().transform((value) => value.toUpperCase()),
});

describe('ZodValidationPipe', () => {
  const pipe = new ZodValidationPipe(schema);

  it('hands the handler the parsed value, not the raw one', () => {
    // Transforms run here, which is why a controller receives a lower-cased
    // address and a coerced number rather than the strings the router parsed.
    expect(pipe.transform({ count: '3', name: 'ada' })).toEqual({ count: 3, name: 'ADA' });
  });

  it('answers 400 rather than letting a bad value reach the handler', () => {
    expect(() => pipe.transform({ count: '0', name: 'ada' })).toThrow(BadRequestException);
  });

  it('names the offending field, the way a bad environment variable does', () => {
    // Same formatter as `validateEnv` and the backfill CLI, so one input error
    // reads like every other in the service — and it says which field, rather
    // than "Bad Request" and nothing else.
    expect(() => pipe.transform({ count: 'nope', name: 'ada' })).toThrow(/count/);
  });

  it('puts the reason in the response body, not only the log', () => {
    const thrown = capture(() => pipe.transform({ count: 'nope', name: 'ada' }));

    expect(thrown?.getResponse()).toMatchObject({ statusCode: 400, error: 'Bad Request' });
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
