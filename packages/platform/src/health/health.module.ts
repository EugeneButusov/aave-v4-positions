import {
  Module,
  type DynamicModule,
  type InjectionToken,
  type ModuleMetadata,
} from '@nestjs/common';

import { HEALTH_INDICATORS, type HealthIndicator } from './health-indicator';
import { HealthController } from './health.controller';
import { HealthService } from './health.service';

export interface HealthModuleOptions extends Pick<ModuleMetadata, 'imports' | 'providers'> {
  /**
   * Tokens resolving to {@link HealthIndicator} instances. Each is resolved
   * through DI — so an indicator can inject a database connection or an RPC
   * client — and the results are collected into the array readiness aggregates.
   *
   * Supply the module that exports them via `imports`, or the classes
   * themselves via `providers`.
   */
  indicators?: InjectionToken[];
}

/**
 * Liveness and readiness probes.
 *
 * Import it plain for probes with no dependency checks, or call
 * {@link HealthModule.forRoot} to register indicators:
 *
 * ```ts
 * HealthModule.forRoot({
 *   imports: [DatabaseModule],
 *   indicators: [DatabaseHealthIndicator],
 * })
 * ```
 *
 * Binding `HEALTH_INDICATORS` from the importing module does not work and never
 * did: `HealthService` resolves inside this module's own injector, so a
 * provider declared alongside the import is not visible to it.
 */
@Module({
  controllers: [HealthController],
  providers: [HealthService],
  exports: [HealthService],
})
export class HealthModule {
  static forRoot(options: HealthModuleOptions = {}): DynamicModule {
    const { imports = [], providers = [], indicators = [] } = options;

    return {
      module: HealthModule,
      imports,
      providers: [
        ...providers,
        {
          provide: HEALTH_INDICATORS,
          // Resolved positionally from `inject`, which is what lets each
          // indicator be a fully injected service rather than a bare value.
          useFactory: (...resolved: HealthIndicator[]) => resolved,
          inject: indicators,
        },
      ],
    };
  }
}
