import type { INestApplication } from '@nestjs/common';
import { Test } from '@nestjs/testing';
import request from 'supertest';
import type { App } from 'supertest/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { AppModule } from '../src/app.module';
import { HealthService } from '@aave-v4-positions/platform';
import { IngestionService } from '../src/ingestion/ingestion.service';

describe('indexer (e2e)', () => {
  let app: INestApplication<App>;

  beforeAll(async () => {
    const moduleRef = await Test.createTestingModule({ imports: [AppModule] }).compile();

    app = moduleRef.createNestApplication({ logger: false });
    await app.init();
  });

  afterAll(async () => {
    await app.close();
  });

  it('boots the whole graph from the validated environment', () => {
    expect(app.get(IngestionService).sourceNames).toEqual([]);
  });

  it('serves liveness', async () => {
    const res = await request(app.getHttpServer()).get('/health/live').expect(200);

    expect(res.body).toMatchObject({ status: 'ok' });
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
});
