import { Test } from '@nestjs/testing';
import { BackfillRunner, BLOCK_PROCESSORS, type BlockProcessor } from '@packages/indexing';
import { describe, expect, it } from 'vitest';

import { BackfillModule } from '../src/backfill/backfill.module';

/**
 * A wiring smoke test, because this is where the command can break silently.
 * Every layer below is unit-tested against fakes, so the one thing left to get
 * wrong is the graph: a runner that resolves with no processors attached would
 * type-check, run, and report success over a range it never touched.
 */
describe('backfill wiring (e2e)', () => {
  it('resolves the runner with the processors the indexer itself registers', async () => {
    const moduleRef = await Test.createTestingModule({ imports: [BackfillModule] }).compile();

    expect(moduleRef.get(BackfillRunner)).toBeInstanceOf(BackfillRunner);

    // Matched loosely: the processor names itself after the Spoke it follows,
    // so pinning the whole string would tie this to one address.
    const processors = moduleRef.get<BlockProcessor[]>(BLOCK_PROCESSORS, { strict: false });
    expect(processors).toHaveLength(1);
    expect(processors[0]?.name).toMatch(/^aave-events\(/);

    await moduleRef.close();
  });
});
