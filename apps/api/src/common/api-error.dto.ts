import { ApiProperty, ApiPropertyOptional } from '@nestjs/swagger';

/**
 * Nest's own error body, declared so a documented failure carries a schema
 * rather than an empty response.
 *
 * Not a custom envelope. The default filter already produces this shape for
 * every `HttpException` in the application, including the ones Nest raises
 * itself for an unknown route — inventing a different one would mean either
 * catching those too or publishing a contract with two error shapes in it.
 */
export class ApiErrorDto {
  @ApiProperty({ example: 400 })
  statusCode!: number;

  @ApiProperty({
    description:
      'What was wrong with the request. Validation failures carry the same ' +
      'formatting the service uses for invalid configuration, so they can span ' +
      'several lines and name each offending field.',
    example: '✖ must be a 20-byte hex address\n  → at user',
  })
  message!: string;

  @ApiPropertyOptional({ example: 'Bad Request' })
  error?: string;
}
