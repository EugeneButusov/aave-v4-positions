import type { INestApplication } from '@nestjs/common';
import { Test } from '@nestjs/testing';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { AppModule } from '../src/app.module';

/**
 * The indexer has no business surface yet, so the only thing worth asserting
 * here is that the module graph composes — a DI misconfiguration in AppModule
 * would otherwise pass the build and fail at boot. Probe behaviour is covered
 * where it lives, in @aave-v4-positions/platform.
 */
describe('indexer (e2e)', () => {
  let app: INestApplication;

  beforeAll(async () => {
    const moduleRef = await Test.createTestingModule({ imports: [AppModule] }).compile();

    app = moduleRef.createNestApplication({ logger: false });
    await app.init();
  });

  afterAll(async () => {
    await app.close();
  });

  it('boots from the validated environment', () => {
    expect(app).toBeDefined();
  });
});
