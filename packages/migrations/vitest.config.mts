import { defineConfig } from 'vitest/config';

// No `resolve.alias` block, unlike every other package here: this one has no
// workspace dependencies to resolve to source, which is the point of it.
export default defineConfig({
  test: {
    name: 'migrations',
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
