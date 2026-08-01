import { fileURLToPath } from 'node:url';

import { defineConfig } from 'vitest/config';

/**
 * Nest reads constructor types from `design:paramtypes`, so the transform must
 * honour `emitDecoratorMetadata`. Vite's own transform does, by reading tsconfig.
 */
export default defineConfig({
  // Resolve the sibling workspace package to its source, not its build output,
  // for the same reason the apps do: otherwise a stale dist is what gets tested.
  resolve: {
    alias: {
      '@packages/ops': fileURLToPath(new URL('../ops/src/index.ts', import.meta.url)),
    },
  },
  test: {
    name: 'indexing',
    globals: true,
    environment: 'node',
    include: ['src/**/*.spec.ts'],
    env: { NODE_ENV: 'test', LOG_LEVEL: 'silent' },
    coverage: {
      provider: 'v8',
      reporter: ['text', 'lcov'],
      include: ['src/**/*.ts'],
      exclude: ['src/**/*.spec.ts', 'src/index.ts', 'src/test-support/**'],
    },
  },
});
