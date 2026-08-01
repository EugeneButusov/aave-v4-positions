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
      '@packages/ops': fileURLToPath(new URL('../../packages/ops/src/index.ts', import.meta.url)),
    },
  },
  test: {
    name: 'indexer',
    globals: true,
    environment: 'node',
    include: ['src/**/*.spec.ts', 'test/**/*.e2e-spec.ts'],
    // Hermetic: NODE_ENV=test makes ConfigModule skip any local .env file, so
    // the variables with no default have to be supplied here or booting the
    // AppModule fails validation. INDEXER_AUTOSTART=false keeps a real indexing
    // loop — and real RPC traffic — out of the test run.
    env: {
      NODE_ENV: 'test',
      LOG_LEVEL: 'silent',
      CHAIN_ID: '1',
      RPC_URLS: 'https://rpc.invalid',
      INDEXER_AUTOSTART: 'false',
    },
    coverage: {
      provider: 'v8',
      reporter: ['text', 'lcov'],
      include: ['src/**/*.ts'],
      exclude: ['src/**/*.spec.ts', 'src/main.ts'],
    },
  },
});
