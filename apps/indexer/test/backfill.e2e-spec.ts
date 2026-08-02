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

    // Both streams and enrichment, and that is the point: a processor added to
    // the daemon and forgotten here would not fail — a backfill would quietly
    // run fewer processors over the range and report success.
    //
    // Enrichment belongs here specifically. Backfilling the genesis range is
    // one of the two ways the token dimension gets filled, because that is the
    // only range that carries an `AddAsset`; the daemon's sweep is the other.
    //
    // Order matters and is asserted: enrichment's fast path reads what the Hub
    // processor wrote earlier in the same dispatch.
    //
    // Pricing is deliberately absent. It is not a processor at all: an oracle's
    // feeds move off chain on their own schedule, so it runs on a timer beside
    // the pipeline rather than inside it — and a backfill, which replays
    // historical blocks and exits, has nothing to ask a current-value source.
    //
    // Matched loosely: the event processors name themselves after the contract
    // they follow, so pinning the whole string would tie this to one address.
    const processors = moduleRef.get<BlockProcessor[]>(BLOCK_PROCESSORS, { strict: false });
    expect(processors.map((p) => p.name.replace(/\(.*/, ''))).toEqual(['aave-spoke', 'aave-hub']);

    await moduleRef.close();
  });
});
