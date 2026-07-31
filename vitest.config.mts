import { defineConfig } from 'vitest/config';

/**
 * Root runner: aggregates every workspace project into a single `vitest run`.
 * Each project keeps its own `vitest.config.mts` so it can also be run in isolation
 * with `pnpm --filter <app> test`.
 */
export default defineConfig({
  test: {
    projects: ['apps/*', 'packages/*'],
  },
});
