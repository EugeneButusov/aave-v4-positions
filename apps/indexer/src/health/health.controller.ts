import { Controller, Get, HttpCode, HttpStatus, ServiceUnavailableException } from '@nestjs/common';

import type { LivenessReport, ReadinessReport } from './health-indicator';
import { HealthService } from './health.service';

/**
 * Deliberately excluded from the global API prefix: probe paths are
 * infrastructure, and pinning them to `/health/*` keeps deployment manifests
 * independent of API versioning.
 */
@Controller('health')
export class HealthController {
  constructor(private readonly health: HealthService) {}

  @Get('live')
  @HttpCode(HttpStatus.OK)
  live(): LivenessReport {
    return this.health.liveness();
  }

  /**
   * Raising the exception rather than writing to the response object keeps this
   * controller free of any HTTP-adapter types; the report is still the whole
   * 503 body, so a probe failure says which dependency is down.
   */
  @Get('ready')
  async ready(): Promise<ReadinessReport> {
    const report = await this.health.readiness();
    if (report.status !== 'ok') throw new ServiceUnavailableException(report);
    return report;
  }
}
