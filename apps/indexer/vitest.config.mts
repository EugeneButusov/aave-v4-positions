import { defineConfig } from 'vitest/config';

/**
 * Nest reads constructor types from `design:paramtypes`, so the transform must
 * honour `emitDecoratorMetadata`. Vite's own transform does, by reading
 * tsconfig — no separate SWC step is needed.
 */
export default defineConfig({
  test: {
    name: 'indexer',
    globals: true,
    environment: 'node',
    include: ['src/**/*.spec.ts', 'test/**/*.e2e-spec.ts'],
    // Hermetic: NODE_ENV=test makes ConfigModule skip any local .env file, so
    // the required RPC_URL comes from here and never from the developer's box.
    env: {
      NODE_ENV: 'test',
      LOG_LEVEL: 'silent',
      RPC_URL: 'http://127.0.0.1:8545',
    },
    coverage: {
      provider: 'v8',
      reporter: ['text', 'lcov'],
      include: ['src/**/*.ts'],
      exclude: ['src/**/*.spec.ts', 'src/main.ts'],
    },
  },
});
