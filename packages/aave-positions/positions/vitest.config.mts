import { fileURLToPath } from 'node:url';

import { defineConfig } from 'vitest/config';

export default defineConfig({
  resolve: {
    alias: {
      '@aave-positions/events': fileURLToPath(new URL('../events/src/index.ts', import.meta.url)),
      '@packages/clickhouse': fileURLToPath(
        new URL('../../clickhouse/src/index.ts', import.meta.url),
      ),
      '@packages/indexing': fileURLToPath(new URL('../../indexing/src/index.ts', import.meta.url)),
      '@packages/migrations': fileURLToPath(
        new URL('../../migrations/src/index.ts', import.meta.url),
      ),
      '@packages/postgres': fileURLToPath(new URL('../../postgres/src/index.ts', import.meta.url)),
    },
  },
  test: {
    name: 'positions',
    globals: true,
    environment: 'node',
    include: ['src/**/*.spec.ts'],
    env: {
      NODE_ENV: 'test',
      LOG_LEVEL: 'silent',
      // The fold is SQL, so its specs run against a real ClickHouse — compose
      // brings one up locally, CI runs it as a service container. Nothing is
      // mocked: a materialized view that does not fire is exactly the bug these
      // specs exist to catch, and no double can reproduce it.
      CLICKHOUSE_URL: process.env['CLICKHOUSE_URL'] ?? 'http://localhost:8123',
      CLICKHOUSE_USER: process.env['CLICKHOUSE_USER'] ?? 'default',
      CLICKHOUSE_PASSWORD: process.env['CLICKHOUSE_PASSWORD'] ?? '',
      // Token metadata is the one table here that is not a fold, and it lives
      // in Postgres — so this project needs both servers. Same default as the
      // indexing package's, because both are pointed at the same local server
      // and CI runs it with `POSTGRES_HOST_AUTH_METHOD=trust`. Without it
      // postgres.js falls back to the OS user, which on a runner is `runner`
      // and fails as "role does not exist" — a confusing way to say unset.
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
