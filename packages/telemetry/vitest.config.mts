import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    name: 'telemetry',
    globals: true,
    environment: 'node',
    include: ['src/**/*.spec.ts'],
    env: { NODE_ENV: 'test' },
    coverage: {
      provider: 'v8',
      reporter: ['text', 'lcov'],
      include: ['src/**/*.ts'],
      // `start.ts` is the preload entry point: it reads the environment and
      // starts a live SDK as a side effect of being required. Exercising it
      // in-process would register global providers for every other suite in
      // the run. It is covered by the compose gate instead.
      exclude: ['src/**/*.spec.ts', 'src/index.ts', 'src/start.ts'],
    },
  },
});
