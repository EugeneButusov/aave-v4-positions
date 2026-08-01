import { fileURLToPath } from 'node:url';

import { defineConfig } from 'vitest/config';

/**
 * Nest reads constructor types from `design:paramtypes`, so the transform must
 * honour `emitDecoratorMetadata`. Vite's own transform does, by reading
 * tsconfig — no separate SWC step is needed.
 */
export default defineConfig({
  // Resolve the workspace packages to their source, not their build output.
  // Without this, editing a package and running tests would silently exercise a
  // stale dist — and watch mode would never see the change at all. One entry per
  // package: nothing derives these from package.json.
  resolve: {
    alias: {
      '@aave-positions/events': fileURLToPath(
        new URL('../../packages/aave-positions/events/src/index.ts', import.meta.url),
      ),
      '@packages/clickhouse': fileURLToPath(
        new URL('../../packages/clickhouse/src/index.ts', import.meta.url),
      ),
      '@packages/indexing': fileURLToPath(
        new URL('../../packages/indexing/src/index.ts', import.meta.url),
      ),
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
