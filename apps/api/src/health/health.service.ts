import { Inject, Injectable, Optional } from '@nestjs/common';

import {
  type CheckResultDto,
  type LivenessResponseDto,
  type ReadinessResponseDto,
} from './dto/health.dto';
import { HEALTH_INDICATORS, type HealthIndicator } from './health-indicator';

@Injectable()
export class HealthService {
  private shuttingDown = false;

  constructor(
    @Optional()
    @Inject(HEALTH_INDICATORS)
    private readonly indicators: HealthIndicator[] = [],
  ) {}

  /**
   * Liveness answers "is this process wedged?", nothing more. It must not touch
   * dependencies — a database outage should drain traffic, not have kubelet
   * restart every replica.
   */
  liveness(): LivenessResponseDto {
    return { status: 'ok', uptimeSeconds: Math.floor(process.uptime()) };
  }

  async readiness(): Promise<ReadinessResponseDto> {
    const checks = await Promise.all(this.indicators.map((i) => this.run(i)));

    if (this.shuttingDown) return { status: 'shutting_down', checks };
    return {
      status: checks.every((c) => c.status === 'up') ? 'ok' : 'degraded',
      checks,
    };
  }

  /**
   * Flipped on SIGTERM so readiness starts failing while the process is still
   * serving. That is what makes a rolling update drain rather than drop
   * in-flight requests.
   */
  beginShutdown(): void {
    this.shuttingDown = true;
  }

  get isShuttingDown(): boolean {
    return this.shuttingDown;
  }

  private async run(indicator: HealthIndicator): Promise<CheckResultDto> {
    try {
      await indicator.check();
      return { name: indicator.name, status: 'up' };
    } catch (error) {
      return {
        name: indicator.name,
        status: 'down',
        error: error instanceof Error ? error.message : String(error),
      };
    }
  }
}
