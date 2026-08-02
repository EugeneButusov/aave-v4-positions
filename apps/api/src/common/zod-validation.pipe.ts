import { BadRequestException, type PipeTransform } from '@nestjs/common';
import { z } from 'zod';

/**
 * Validates a request part against a Zod schema, and answers 400 when it does
 * not fit.
 *
 * Zod rather than `class-validator`, because configuration and CLI arguments
 * already validate this way: one idiom in the repo instead of two, no new
 * dependency, and a bad query parameter reads exactly like a bad environment
 * variable because `z.prettifyError` formats both.
 *
 * The schema's transforms run here, so a handler receives the parsed value —
 * a lower-cased address, a coerced number — rather than the raw strings the
 * router hands over.
 *
 * Request shapes are documented for OpenAPI with `@ApiQuery`/`@ApiParam` rather
 * than derived from these schemas. The two could be generated from one source,
 * and that is a real trade to revisit; today it is one endpoint, and the
 * decorators say what a caller may send while the schema says what is accepted.
 */
export class ZodValidationPipe<TSchema extends z.ZodType> implements PipeTransform<
  unknown,
  z.output<TSchema>
> {
  constructor(private readonly schema: TSchema) {}

  transform(value: unknown): z.output<TSchema> {
    const result = this.schema.safeParse(value);
    if (!result.success) {
      throw new BadRequestException(z.prettifyError(result.error));
    }
    return result.data;
  }
}
