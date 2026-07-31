import { defineConfig } from 'vitest/config';

/**
 * Root runner: aggregates every workspace app into a single `vitest run`.
 * Each app keeps its own `vitest.config.mts` so it can also be run in isolation
 * with `pnpm --filter <app> test`.
 */
export default defineConfig({
  test: {
    projects: ['apps/*'],
  },
});
