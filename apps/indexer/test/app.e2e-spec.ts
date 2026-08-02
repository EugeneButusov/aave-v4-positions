import type { INestApplication } from '@nestjs/common';
import { Test } from '@nestjs/testing';
import {
  BLOCK_HEADER_STORE,
  CURSOR_STORE,
  PostgresBlockHeaderStore,
  PostgresCursorStore,
} from '@packages/indexing';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { AppModule } from '../src/app.module';

/**
 * The indexer has no business surface yet, so the only thing worth asserting
 * here is that the module graph composes — a DI misconfiguration in AppModule
 * would otherwise pass the build and fail at boot. Probe behaviour is covered
 * where it lives, in @packages/ops.
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

  it('resolves both state stores to their durable adapters', () => {
    // The failure this exists to catch is shipping with the in-memory adapters
    // still wired. Everything would boot, every other test would pass, and the
    // indexer would silently start from INDEXER_START_BLOCK on every restart —
    // which is exactly the behaviour it is supposed to have stopped having.
    // Needs no server: nothing here issues a query.
    expect(app.get(CURSOR_STORE, { strict: false })).toBeInstanceOf(PostgresCursorStore);
    expect(app.get(BLOCK_HEADER_STORE, { strict: false })).toBeInstanceOf(PostgresBlockHeaderStore);
  });
});
