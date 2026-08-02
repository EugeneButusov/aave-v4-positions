import { fileURLToPath } from 'node:url';

import { defineConfig } from 'vitest/config';

/**
 * Nest reads constructor types from `design:paramtypes`, so the transform must
 * honour `emitDecoratorMetadata`. Vite's own transform does, by reading
 * tsconfig — no separate SWC step is needed.
 */
export default defineConfig({
  // Resolve the workspace package to its source, not its build output. Without
  // this, editing packages/ops and running tests would silently exercise a
  // stale dist — and watch mode would never see the change at all.
  resolve: {
    alias: {
      '@aave-positions/positions': fileURLToPath(
        new URL('../../packages/aave-positions/positions/src/index.ts', import.meta.url),
      ),
      '@packages/token-metadata': fileURLToPath(
        new URL('../../packages/token-metadata/src/index.ts', import.meta.url),
      ),
      '@packages/clickhouse': fileURLToPath(
        new URL('../../packages/clickhouse/src/index.ts', import.meta.url),
      ),
      '@packages/indexing': fileURLToPath(
        new URL('../../packages/indexing/src/index.ts', import.meta.url),
      ),
      '@packages/ops': fileURLToPath(new URL('../../packages/ops/src/index.ts', import.meta.url)),
      '@packages/postgres': fileURLToPath(
        new URL('../../packages/postgres/src/index.ts', import.meta.url),
      ),
    },
  },
  test: {
    name: 'api',
    globals: true,
    environment: 'node',
    include: ['src/**/*.spec.ts', 'test/**/*.e2e-spec.ts'],
    // Hermetic: no database is reached. Neither client opens a socket at boot,
    // and the specs override the two stores — what the real ones do is pinned
    // where they live, against real servers, and no double could reproduce it.
    env: {
      NODE_ENV: 'test',
      LOG_LEVEL: 'silent',
      // Long enough to satisfy the codec, which refuses a guessable key.
      POSITIONS_CURSOR_SECRET: 'spec-cursor-secret'.padEnd(32, '.'),
    },
    coverage: {
      provider: 'v8',
      reporter: ['text', 'lcov'],
      include: ['src/**/*.ts'],
      exclude: ['src/**/*.spec.ts', 'src/main.ts'],
    },
  },
});
