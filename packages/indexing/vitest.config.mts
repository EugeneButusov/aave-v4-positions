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
      '@packages/postgres': fileURLToPath(new URL('../postgres/src/index.ts', import.meta.url)),
    },
  },
  test: {
    name: 'indexing',
    globals: true,
    environment: 'node',
    include: ['src/**/*.spec.ts'],
    env: {
      NODE_ENV: 'test',
      LOG_LEVEL: 'silent',
      // The two Postgres adapter specs go against a real server — what they
      // assert is that the SQL means what the port says, the data-modifying CTE
      // above all. Each takes a schema of its own, so they run beside the rest
      // of this project's files rather than serialising it.
      POSTGRES_URL: process.env['POSTGRES_URL'] ?? 'postgres://postgres@localhost:5432/postgres',
    },
    coverage: {
      provider: 'v8',
      reporter: ['text', 'lcov'],
      include: ['src/**/*.ts'],
      exclude: ['src/**/*.spec.ts', 'src/index.ts', 'src/test-support/**'],
    },
  },
});
