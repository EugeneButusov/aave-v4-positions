import { Controller, Get, HttpCode, HttpStatus, ServiceUnavailableException } from '@nestjs/common';
import {
  ApiOkResponse,
  ApiOperation,
  ApiServiceUnavailableResponse,
  ApiTags,
} from '@nestjs/swagger';

import { LivenessResponseDto, ReadinessResponseDto } from './dto/health.dto';
import { HealthService } from './health.service';

/**
 * Deliberately excluded from the global API prefix: probe paths are
 * infrastructure, and pinning them to `/health/*` keeps deployment manifests
 * independent of API versioning.
 */
@ApiTags('health')
@Controller('health')
export class HealthController {
  constructor(private readonly health: HealthService) {}

  @Get('live')
  @HttpCode(HttpStatus.OK)
  @ApiOperation({
    summary: 'Liveness probe',
    description:
      'Reports only whether the process is wedged. Never touches a dependency — ' +
      'a database outage should drain traffic, not restart every replica.',
  })
  @ApiOkResponse({ type: LivenessResponseDto })
  live(): LivenessResponseDto {
    return this.health.liveness();
  }

  /**
   * Raising the exception rather than writing to the response object keeps this
   * controller free of any HTTP-adapter types; the report is still the whole
   * 503 body, so a probe failure says which dependency is down.
   */
  @Get('ready')
  @ApiOperation({
    summary: 'Readiness probe',
    description:
      'Aggregates every registered dependency check. Also fails while the pod is ' +
      'draining, so a rolling update removes it from the load balancer before it ' +
      'stops serving.',
  })
  @ApiOkResponse({ type: ReadinessResponseDto })
  @ApiServiceUnavailableResponse({
    type: ReadinessResponseDto,
    description: 'A dependency is down, or the pod is draining.',
  })
  async ready(): Promise<ReadinessResponseDto> {
    const report = await this.health.readiness();
    if (report.status !== 'ok') throw new ServiceUnavailableException(report);
    return report;
  }
}
