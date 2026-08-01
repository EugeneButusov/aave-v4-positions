import { defineConfig } from 'vitest/config';

/**
 * Root runner: aggregates every workspace project into a single `vitest run`.
 * Each project keeps its own `vitest.config.mts` so it can also be run in isolation
 * with `pnpm --filter <app> test`.
 */
export default defineConfig({
  test: {
    // Matched against config files rather than directories: domain packages nest
    // one level, and a bare `packages/*/*` would match every file inside a
    // flat package rather than the nested packages it is meant to reach.
    projects: [
      'apps/*/vitest.config.mts',
      'packages/*/vitest.config.mts',
      'packages/*/*/vitest.config.mts',
    ],
  },
});
