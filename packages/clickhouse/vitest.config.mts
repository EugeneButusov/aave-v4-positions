import { fileURLToPath } from 'node:url';

import { defineConfig } from 'vitest/config';

export default defineConfig({
  resolve: {
    alias: {
      '@packages/migrations': fileURLToPath(new URL('../migrations/src/index.ts', import.meta.url)),
      '@packages/ops': fileURLToPath(new URL('../ops/src/index.ts', import.meta.url)),
    },
  },
  test: {
    name: 'clickhouse',
    globals: true,
    environment: 'node',
    include: ['src/**/*.spec.ts'],
    env: {
      NODE_ENV: 'test',
      LOG_LEVEL: 'silent',
    },
    coverage: {
      provider: 'v8',
      reporter: ['text', 'lcov'],
      include: ['src/**/*.ts'],
      exclude: ['src/**/*.spec.ts', 'src/index.ts'],
    },
  },
});
