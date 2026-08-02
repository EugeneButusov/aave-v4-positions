import { fileURLToPath } from 'node:url';

import { defineConfig } from 'vitest/config';

export default defineConfig({
  resolve: {
    alias: {
      '@aave-positions/events': fileURLToPath(new URL('../events/src/index.ts', import.meta.url)),
      // The subpath first: Vite matches aliases by prefix, in order, so the
      // bare entry below would otherwise rewrite it to `index.ts/test-support`.
      '@aave-positions/positions/test-support': fileURLToPath(
        new URL('../positions/src/test-support/index.ts', import.meta.url),
      ),
      '@aave-positions/positions': fileURLToPath(
        new URL('../positions/src/index.ts', import.meta.url),
      ),
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
    name: 'enrichment',
    globals: true,
    environment: 'node',
    include: ['src/**/*.spec.ts'],
    env: {
      NODE_ENV: 'test',
      LOG_LEVEL: 'silent',
      // The listing sources read the folds, so their specs run against a real
      // ClickHouse — compose brings one up locally, CI runs it as a service
      // container. Nothing is mocked: what these prove is that the SQL finds
      // what it claims to, and no double can tell us that.
      CLICKHOUSE_URL: process.env['CLICKHOUSE_URL'] ?? 'http://localhost:8123',
      CLICKHOUSE_USER: process.env['CLICKHOUSE_USER'] ?? 'default',
      CLICKHOUSE_PASSWORD: process.env['CLICKHOUSE_PASSWORD'] ?? '',
      // Both stores here are in Postgres, so this project needs both servers.
      // Same default as the indexing package's, because both are pointed at
      // the same local server
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
