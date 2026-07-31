import { ApiProperty, ApiPropertyOptional } from '@nestjs/swagger';

/**
 * Response shapes for the probe endpoints.
 *
 * These are classes rather than interfaces because interfaces are erased before
 * runtime and OpenAPI schema generation reads decorator metadata. Every property
 * is annotated explicitly rather than relying on the `@nestjs/swagger` CLI
 * plugin: the plugin only runs through the Nest CLI build, so under Vitest the
 * generated document would silently lose properties and the contract test would
 * be asserting something the running service does not produce.
 */

export const CHECK_STATUSES = ['up', 'down'] as const;
export type CheckStatus = (typeof CHECK_STATUSES)[number];

export const READINESS_STATUSES = ['ok', 'degraded', 'shutting_down'] as const;
export type ReadinessStatus = (typeof READINESS_STATUSES)[number];

export class LivenessResponseDto {
  @ApiProperty({
    enum: ['ok'],
    description: 'Always `ok`. A wedged process fails to respond at all.',
  })
  status!: 'ok';

  @ApiProperty({ example: 3421, description: 'Seconds since the process started.' })
  uptimeSeconds!: number;
}

export class CheckResultDto {
  @ApiProperty({ example: 'database', description: 'Name of the dependency checked.' })
  name!: string;

  @ApiProperty({ enum: CHECK_STATUSES })
  status!: CheckStatus;

  @ApiPropertyOptional({
    example: 'connection refused',
    description: 'Reason the check failed. Absent when the dependency is up.',
  })
  error?: string;
}

export class ReadinessResponseDto {
  @ApiProperty({
    enum: READINESS_STATUSES,
    description:
      '`degraded` when a dependency is down, `shutting_down` while the pod is ' +
      'draining. Both are served with a 503.',
  })
  status!: ReadinessStatus;

  @ApiProperty({ type: [CheckResultDto] })
  checks!: CheckResultDto[];
}
