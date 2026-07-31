import { Test } from '@nestjs/testing';
import { beforeEach, describe, expect, it } from 'vitest';

import { HEALTH_INDICATORS, type HealthIndicator } from './health-indicator';
import { HealthService } from './health.service';

const indicator = (name: string, fail?: string): HealthIndicator => ({
  name,
  check: () => {
    if (fail !== undefined) throw new Error(fail);
  },
});

async function buildService(indicators?: HealthIndicator[]): Promise<HealthService> {
  const moduleRef = await Test.createTestingModule({
    providers: [
      HealthService,
      ...(indicators ? [{ provide: HEALTH_INDICATORS, useValue: indicators }] : []),
    ],
  }).compile();

  return moduleRef.get(HealthService);
}

describe('HealthService', () => {
  describe('liveness', () => {
    it('reports ok without consulting any dependency', async () => {
      const service = await buildService([indicator('db', 'connection refused')]);

      expect(service.liveness().status).toBe('ok');
    });
  });

  describe('readiness', () => {
    it('is ok with no indicators registered', async () => {
      const service = await buildService();

      await expect(service.readiness()).resolves.toEqual({ status: 'ok', checks: [] });
    });

    it('reports every indicator that is up', async () => {
      const service = await buildService([indicator('db'), indicator('rpc')]);

      await expect(service.readiness()).resolves.toEqual({
        status: 'ok',
        checks: [
          { name: 'db', status: 'up' },
          { name: 'rpc', status: 'up' },
        ],
      });
    });

    it('degrades on a single failing indicator and surfaces its reason', async () => {
      const service = await buildService([indicator('db', 'connection refused'), indicator('rpc')]);

      await expect(service.readiness()).resolves.toEqual({
        status: 'degraded',
        checks: [
          { name: 'db', status: 'down', error: 'connection refused' },
          { name: 'rpc', status: 'up' },
        ],
      });
    });
  });

  describe('shutdown', () => {
    let service: HealthService;

    beforeEach(async () => {
      service = await buildService([indicator('db')]);
    });

    it('fails readiness once draining has started, even while dependencies are up', async () => {
      service.beginShutdown();

      const report = await service.readiness();
      expect(report.status).toBe('shutting_down');
      expect(report.checks).toEqual([{ name: 'db', status: 'up' }]);
    });

    it('keeps liveness ok while draining so kubelet does not restart the pod', () => {
      service.beginShutdown();

      expect(service.liveness().status).toBe('ok');
    });
  });
});
