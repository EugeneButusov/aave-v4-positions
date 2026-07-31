import type { INestApplication } from '@nestjs/common';
import { Test } from '@nestjs/testing';
import request from 'supertest';
import type { App } from 'supertest/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { AppModule } from '../src/app.module';
import { HealthService } from '../src/health/health.service';

describe('api (e2e)', () => {
  let app: INestApplication<App>;

  beforeAll(async () => {
    const moduleRef = await Test.createTestingModule({ imports: [AppModule] }).compile();

    app = moduleRef.createNestApplication({ logger: false });
    app.setGlobalPrefix('api', { exclude: ['health/live', 'health/ready'] });
    await app.init();
  });

  afterAll(async () => {
    await app.close();
  });

  it('serves liveness outside the global prefix', async () => {
    const res = await request(app.getHttpServer()).get('/health/live').expect(200);

    expect(res.body).toMatchObject({ status: 'ok' });
    expect(res.body.uptimeSeconds).toBeGreaterThanOrEqual(0);
  });

  it('serves readiness with no dependencies registered yet', async () => {
    await request(app.getHttpServer())
      .get('/health/ready')
      .expect(200)
      .expect({ status: 'ok', checks: [] });
  });

  it('answers 503 on readiness once draining begins', async () => {
    app.get(HealthService).beginShutdown();

    await request(app.getHttpServer())
      .get('/health/ready')
      .expect(503)
      .expect({ status: 'shutting_down', checks: [] });
  });

  it('does not expose probes under the prefix', async () => {
    await request(app.getHttpServer()).get('/api/health/live').expect(404);
  });
});
