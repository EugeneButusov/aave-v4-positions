import { fileURLToPath } from 'node:url';

import { defineConfig } from 'vitest/config';

export default defineConfig({
  resolve: {
    alias: {
      '@packages/ops': fileURLToPath(new URL('../ops/src/index.ts', import.meta.url)),
    },
  },
  test: {
    name: 'postgres',
    globals: true,
    environment: 'node',
    include: ['src/**/*.spec.ts'],
    env: {
      NODE_ENV: 'test',
      LOG_LEVEL: 'silent',
      // The runner spec goes against a real server, like the ClickHouse store
      // specs: what it asserts is that the DDL executes and that a failed
      // migration rolls back, and neither is something a fake can tell you.
      POSTGRES_URL: process.env['POSTGRES_URL'] ?? 'postgres://postgres@localhost:5432/postgres',
    },
    coverage: {
      provider: 'v8',
      reporter: ['text', 'lcov'],
      include: ['src/**/*.ts'],
      exclude: ['src/**/*.spec.ts', 'src/index.ts'],
    },
  },
});
