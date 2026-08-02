import { Test } from '@nestjs/testing';
import { describe, expect, it } from 'vitest';

import { PriceModule } from './price.module';
import { ReservePricer } from './reserve-pricer';

/**
 * That the command's graph resolves at all.
 *
 * **This spec exists because its absence has already cost something.** A
 * one-shot command is a provider in its own module, and Nest resolves a
 * provider's dependencies from the module it is *declared* in — so a port the
 * feature module provides but does not `export` compiles perfectly and fails
 * only when someone runs the command. Nothing else here would catch it:
 * `typecheck` sees the imports resolve, and no other spec constructs this
 * module.
 *
 * Hermetic. Neither database client opens a socket at construction and the
 * chain client is lazy, so nothing is reached — `RPC_URLS` points at
 * `rpc.invalid` from the vitest config and stays unused.
 */
describe('PriceModule', () => {
  it('resolves the pricer and every port it injects', async () => {
    const moduleRef = await Test.createTestingModule({ imports: [PriceModule] }).compile();

    // Constructing it is the assertion: Nest throws here, not at `compile()`,
    // if any of the four injected tokens is unreachable from this module.
    expect(moduleRef.get(ReservePricer)).toBeInstanceOf(ReservePricer);

    await moduleRef.close();
  });
});
