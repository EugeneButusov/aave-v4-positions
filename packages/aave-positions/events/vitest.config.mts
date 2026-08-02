import { fileURLToPath } from 'node:url';

import { defineConfig } from 'vitest/config';

export default defineConfig({
  resolve: {
    alias: {
      '@packages/clickhouse': fileURLToPath(
        new URL('../../clickhouse/src/index.ts', import.meta.url),
      ),
      '@packages/indexing': fileURLToPath(new URL('../../indexing/src/index.ts', import.meta.url)),
      '@packages/migrations': fileURLToPath(
        new URL('../../migrations/src/index.ts', import.meta.url),
      ),
      '@packages/ops': fileURLToPath(new URL('../../ops/src/index.ts', import.meta.url)),
    },
  },
  test: {
    name: 'events',
    globals: true,
    environment: 'node',
    include: ['src/**/*.spec.ts'],
    env: {
      NODE_ENV: 'test',
      LOG_LEVEL: 'silent',
      // The store specs run against a real ClickHouse — compose brings one up
      // locally, CI runs it as a service container. Nothing here is mocked:
      // the whole point of those specs is that the SQL executes.
      CLICKHOUSE_URL: process.env['CLICKHOUSE_URL'] ?? 'http://localhost:8123',
      CLICKHOUSE_USER: process.env['CLICKHOUSE_USER'] ?? 'default',
      CLICKHOUSE_PASSWORD: process.env['CLICKHOUSE_PASSWORD'] ?? '',
    },
    coverage: {
      provider: 'v8',
      reporter: ['text', 'lcov'],
      include: ['src/**/*.ts'],
      exclude: ['src/**/*.spec.ts', 'src/index.ts'],
    },
  },
});
