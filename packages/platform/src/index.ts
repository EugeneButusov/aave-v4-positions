/**
 * Cross-service Nest infrastructure.
 *
 * Anything in here is used identically by more than one service. It holds no
 * Aave domain logic — probes, readiness indicators and the shutdown sequence
 * would look the same in any Kubernetes-deployed Nest service.
 */

export { HEALTH_INDICATORS, type HealthIndicator } from './health/health-indicator';

export {
  CHECK_STATUSES,
  CheckResultDto,
  LivenessResponseDto,
  READINESS_STATUSES,
  ReadinessResponseDto,
  type CheckStatus,
  type ReadinessStatus,
} from './health/health.dto';

export { HealthController } from './health/health.controller';
export { HealthModule } from './health/health.module';
export { HealthService } from './health/health.service';

export { installGracefulShutdown } from './lifecycle/graceful-shutdown';
