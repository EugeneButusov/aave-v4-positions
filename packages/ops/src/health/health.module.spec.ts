import { Injectable, Module } from '@nestjs/common';
import { Test } from '@nestjs/testing';
import { describe, expect, it } from 'vitest';

import type { HealthIndicator } from './health-indicator';
import { HealthModule } from './health.module';
import { HealthService } from './health.service';

/** An indicator with a real dependency, to prove they resolve through DI. */
@Injectable()
class Connection {
  reachable = true;
}

@Injectable()
class DatabaseIndicator implements HealthIndicator {
  readonly name = 'database';
  constructor(private readonly connection: Connection) {}
  check(): void {
    if (!this.connection.reachable) throw new Error('connection refused');
  }
}

@Module({ providers: [Connection, DatabaseIndicator], exports: [DatabaseIndicator] })
class DatabaseModule {}

const readinessOf = async (moduleImport: Parameters<typeof Test.createTestingModule>[0]) => {
  const ref = await Test.createTestingModule(moduleImport).compile();
  return { ref, report: await ref.get(HealthService, { strict: false }).readiness() };
};

describe('HealthModule', () => {
  it('serves probes with no indicators when imported plain', async () => {
    const { report } = await readinessOf({ imports: [HealthModule] });

    expect(report).toEqual({ status: 'ok', checks: [] });
  });

  it('serves probes with no indicators when forRoot is called bare', async () => {
    const { report } = await readinessOf({ imports: [HealthModule.forRoot()] });

    expect(report).toEqual({ status: 'ok', checks: [] });
  });

  it('resolves an indicator declared locally', async () => {
    const { report } = await readinessOf({
      imports: [
        HealthModule.forRoot({
          providers: [Connection, DatabaseIndicator],
          indicators: [DatabaseIndicator],
        }),
      ],
    });

    expect(report).toEqual({ status: 'ok', checks: [{ name: 'database', status: 'up' }] });
  });

  it('resolves an indicator exported by an imported module', async () => {
    const { report } = await readinessOf({
      imports: [
        HealthModule.forRoot({ imports: [DatabaseModule], indicators: [DatabaseIndicator] }),
      ],
    });

    expect(report.checks).toEqual([{ name: 'database', status: 'up' }]);
  });

  it('reports the dependency down when its injected collaborator fails', async () => {
    const ref = await Test.createTestingModule({
      imports: [
        HealthModule.forRoot({ imports: [DatabaseModule], indicators: [DatabaseIndicator] }),
      ],
    }).compile();

    ref.get(Connection, { strict: false }).reachable = false;

    await expect(ref.get(HealthService, { strict: false }).readiness()).resolves.toEqual({
      status: 'degraded',
      checks: [{ name: 'database', status: 'down', error: 'connection refused' }],
    });
  });

  it('aggregates several indicators in registration order', async () => {
    const rpc: HealthIndicator = { name: 'rpc', check: () => undefined };

    const { report } = await readinessOf({
      imports: [
        HealthModule.forRoot({
          imports: [DatabaseModule],
          providers: [{ provide: 'RPC_INDICATOR', useValue: rpc }],
          indicators: [DatabaseIndicator, 'RPC_INDICATOR'],
        }),
      ],
    });

    expect(report.checks.map((c) => c.name)).toEqual(['database', 'rpc']);
  });
});
