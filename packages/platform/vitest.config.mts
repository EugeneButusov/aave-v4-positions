import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    name: 'platform',
    globals: true,
    environment: 'node',
    include: ['src/**/*.spec.ts'],
    env: { NODE_ENV: 'test', LOG_LEVEL: 'silent' },
    coverage: {
      provider: 'v8',
      reporter: ['text', 'lcov'],
      include: ['src/**/*.ts'],
      exclude: ['src/**/*.spec.ts', 'src/index.ts'],
    },
  },
});
