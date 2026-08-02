import { Test } from '@nestjs/testing';
import type { Sql } from 'postgres';
import { describe, expect, it } from 'vitest';

import { PostgresHealthIndicator } from './postgres.health-indicator';
import { PostgresModule } from './postgres.module';
import { POSTGRES_CLIENT } from './postgres.options';

describe('PostgresModule', () => {
  it('builds the client without reaching the database', async () => {
    const moduleRef = await Test.createTestingModule({
      imports: [
        PostgresModule.forRootAsync({
          // Deliberately unroutable. postgres.js connects on the first query, so
          // a module that resolved without throwing here is the proof that boot
          // does not depend on the database being up — a pod that crash-looped
          // instead would never serve the readiness probe that says why.
          useFactory: () => ({ url: 'postgres://nobody@240.0.0.1:5432/nothing' }),
        }),
      ],
    }).compile();

    const client = moduleRef.get<Sql>(POSTGRES_CLIENT, { strict: false });
    expect(moduleRef.get(PostgresHealthIndicator, { strict: false })).toBeInstanceOf(
      PostgresHealthIndicator,
    );

    await moduleRef.close();
    // Nest closes nothing here on purpose — no module in this repo registers a
    // shutdown hook for its database client — so the spec ends it itself, or
    // the worker would sit on an open pool.
    await client.end();
  });
});
