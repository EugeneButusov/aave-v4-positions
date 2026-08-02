import type { INestApplication } from '@nestjs/common';
import { Test, type TestingModule } from '@nestjs/testing';
import request from 'supertest';
import type { App } from 'supertest/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { POSITION_STORE, ClickHousePositionStore } from '@aave-positions/positions';
import { SYNC_STATUS_STORE, PostgresSyncStatusStore } from '@packages/indexing';

import { AppModule } from '../src/app.module';
import { httpSetup } from '../src/http.setup';

/**
 * Covers how the app is wired and mounted. What the endpoint answers is in
 * positions.e2e-spec.ts; probe behaviour is tested where it lives, in
 * @packages/ops.
 */
describe('api (e2e)', () => {
  let app: INestApplication<App>;
  let moduleRef: TestingModule;

  beforeAll(async () => {
    // No overrides here, deliberately — see the wiring assertions below.
    moduleRef = await Test.createTestingModule({ imports: [AppModule] }).compile();

    app = moduleRef.createNestApplication({ logger: false });
    httpSetup(app, { globalPrefix: 'api' });
    await app.init();
  });

  afterAll(async () => {
    await app.close();
  });

  it('answers 404 for an unknown route rather than hanging', async () => {
    await request(app.getHttpServer()).get('/api/nope').expect(404);
  });

  it.each([
    ['positions', POSITION_STORE, ClickHousePositionStore],
    ['sync status', SYNC_STATUS_STORE, PostgresSyncStatusStore],
  ])('resolves the %s store to the real adapter', (_case, token, adapter) => {
    // The failure this guards is shipping with a fake wired: every route still
    // answers, every other test still passes, and the service serves whatever
    // the double was seeded with.
    expect(moduleRef.get(token)).toBeInstanceOf(adapter);
  });

  it('leaves the probes unversioned and outside the prefix', async () => {
    // The guard on `enableVersioning`. Passing `defaultVersion` would apply a
    // version to every controller — the exclude list strips the prefix but not
    // a version segment — and the probes would move to `/v1/health/live`,
    // taking the compose healthcheck and every deployment manifest with them.
    await request(app.getHttpServer()).get('/health/live').expect(200);
    await request(app.getHttpServer()).get('/v1/health/live').expect(404);
  });
});
