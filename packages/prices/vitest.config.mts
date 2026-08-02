import { fileURLToPath } from 'node:url';

import { defineConfig } from 'vitest/config';

export default defineConfig({
  resolve: {
    alias: {
      // The subpath first: Vite matches aliases by prefix, in order, so the
      // bare entry below would otherwise rewrite it to `index.ts/test-support`.
      '@aave-positions/positions/test-support': fileURLToPath(
        new URL('../aave-positions/positions/src/test-support/index.ts', import.meta.url),
      ),
      '@aave-positions/positions': fileURLToPath(
        new URL('../aave-positions/positions/src/index.ts', import.meta.url),
      ),
      '@aave-positions/events': fileURLToPath(
        new URL('../aave-positions/events/src/index.ts', import.meta.url),
      ),
      '@packages/clickhouse': fileURLToPath(new URL('../clickhouse/src/index.ts', import.meta.url)),
      '@packages/indexing': fileURLToPath(new URL('../indexing/src/index.ts', import.meta.url)),
      '@packages/migrations': fileURLToPath(new URL('../migrations/src/index.ts', import.meta.url)),
      '@packages/postgres': fileURLToPath(new URL('../postgres/src/index.ts', import.meta.url)),
    },
  },
  test: {
    name: 'prices',
    globals: true,
    environment: 'node',
    include: ['src/**/*.spec.ts'],
    env: {
      NODE_ENV: 'test',
      LOG_LEVEL: 'silent',
      // The reserve listing reads the Spoke registry fold, so its spec runs against
      // a real ClickHouse; the store is Postgres. Nothing is mocked — what these
      // prove is that the SQL finds what it claims to, and no double can say that.
      CLICKHOUSE_URL: process.env['CLICKHOUSE_URL'] ?? 'http://localhost:8123',
      CLICKHOUSE_USER: process.env['CLICKHOUSE_USER'] ?? 'default',
      CLICKHOUSE_PASSWORD: process.env['CLICKHOUSE_PASSWORD'] ?? '',
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
