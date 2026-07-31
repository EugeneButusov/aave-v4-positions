import { fileURLToPath } from 'node:url';

import { defineConfig } from 'vitest/config';

/**
 * Nest reads constructor types from `design:paramtypes`, so the transform must
 * honour `emitDecoratorMetadata`. Vite's own transform does, by reading
 * tsconfig — no separate SWC step is needed. `src/toolchain.spec.ts` asserts
 * that, so the day it stops being true the failure is legible.
 */
export default defineConfig({
  // Resolve the workspace package to its source, not its build output. Without
  // this, editing packages/platform and running tests would silently exercise a
  // stale dist — and watch mode would never see the change at all.
  resolve: {
    alias: {
      '@aave-v4-positions/platform': fileURLToPath(
        new URL('../../packages/platform/src/index.ts', import.meta.url),
      ),
    },
  },
  test: {
    name: 'api',
    globals: true,
    environment: 'node',
    include: ['src/**/*.spec.ts', 'test/**/*.e2e-spec.ts'],
    // Hermetic: NODE_ENV=test makes ConfigModule skip any local .env file.
    env: { NODE_ENV: 'test', LOG_LEVEL: 'silent' },
    coverage: {
      provider: 'v8',
      reporter: ['text', 'lcov'],
      include: ['src/**/*.ts'],
      exclude: ['src/**/*.spec.ts', 'src/main.ts'],
    },
  },
});
