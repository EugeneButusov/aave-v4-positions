/**
 * Cross-service Nest infrastructure.
 *
 * Anything in here is used identically by more than one service. It holds no
 * Aave domain logic — probes, readiness indicators and the shutdown sequence
 * would look the same in any Kubernetes-deployed Nest service.
 */

export { HEALTH_INDICATORS, type HealthIndicator } from './health/health-indicator';

export {
  CheckResultDto,
  LivenessResponseDto,
  ReadinessResponseDto,
  type CheckStatus,
  type ReadinessStatus,
} from './health/health.dto';

export { HealthController } from './health/health.controller';
export { HealthModule } from './health/health.module';
export { HealthService } from './health/health.service';

export { LoggingModule, type LoggingAsyncOptions } from './logging/logging.module';
export {
  LOG_LEVELS,
  REQUEST_ID_HEADER,
  buildLoggingParams,
  type LoggingOptions,
  type LogLevel,
} from './logging/logging.options';

// Re-exported so services never import nestjs-pino directly.
export { Logger } from 'nestjs-pino';

export { installGracefulShutdown } from './lifecycle/graceful-shutdown';
