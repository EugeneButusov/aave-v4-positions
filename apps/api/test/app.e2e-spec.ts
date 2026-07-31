import type { INestApplication } from '@nestjs/common';
import { Test } from '@nestjs/testing';
import request from 'supertest';
import type { App } from 'supertest/types';
import { afterAll, beforeAll, describe, it } from 'vitest';

import { AppModule } from '../src/app.module';

/**
 * Covers the api's own surface and how it is mounted. Probe behaviour is tested
 * where it lives, in @aave-v4-positions/platform.
 */
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

  it('serves the stub endpoint under the global prefix', async () => {
    await request(app.getHttpServer())
      .get('/api/hello')
      .expect(200)
      .expect({ message: 'aave-v4-positions api' });
  });

  it('does not serve it unprefixed', async () => {
    await request(app.getHttpServer()).get('/hello').expect(404);
  });

  it('answers 404 for an unknown route rather than hanging', async () => {
    await request(app.getHttpServer()).get('/api/nope').expect(404);
  });
});
