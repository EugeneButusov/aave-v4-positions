import 'reflect-metadata';

import { describe, expect, it } from 'vitest';

import { HealthController, HealthService } from '@aave-v4-positions/platform';

/**
 * Guard on the test toolchain itself, not on application behaviour.
 *
 * Nest resolves constructor dependencies from the `design:paramtypes` metadata
 * that `emitDecoratorMetadata` produces. Vite's transform emits it today by
 * reading tsconfig, which is why this project needs no separate SWC transform
 * step. If a future toolchain stops emitting it, every DI-backed test would
 * fail at once with an opaque "can't resolve dependencies" error — this test
 * fails first, and says why.
 */
describe('decorator metadata emission', () => {
  it('records constructor parameter types on an injectable controller', () => {
    const paramTypes: unknown = Reflect.getMetadata('design:paramtypes', HealthController);

    expect(paramTypes).toEqual([HealthService]);
  });
});
