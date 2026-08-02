import { CLICKHOUSE_CLIENT, ClickHouseHealthIndicator } from '@packages/clickhouse';
import { LOG_READER, type BlockProcessor, type LogReader } from '@packages/indexing';
import { Injectable, Module } from '@nestjs/common';
import { Test } from '@nestjs/testing';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { AaveEventProcessor } from './aave-event-processor';
import { SPOKE_EVENT_STORE, type EventStore } from './store/event-store';
import { SPOKE_EVENT_PROCESSOR, SpokeEventsModule } from './spoke-events.module';

const SPOKE = '0x94e7a5dcbe816e498b89ab752661904e2f56c485';
const OTHER_SPOKE = '0x0000000000000000000000000000000000000abc';

@Injectable()
class Settings {
  readonly spoke = SPOKE;
}

/** Stands in for the application's ConfigModule: options arrive through DI. */
@Module({ providers: [Settings], exports: [Settings] })
class SettingsModule {}

function spokeEvents(spoke: string = SPOKE, token?: symbol) {
  return SpokeEventsModule.forRootAsync({
    imports: [SettingsModule],
    inject: [Settings],
    useFactory: () => ({
      chainId: 1,
      spoke,
      rpc: { rpcUrls: ['https://rpc.invalid'], rpcTimeoutMs: 1_000 },
      clickhouse: {
        url: 'http://clickhouse.invalid',
        database: 'aave',
        username: 'aave',
        password: '',
      },
    }),
    ...(token && { token }),
  });
}

describe('SpokeEventsModule', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('exports a processor built for the configured Spoke', async () => {
    const moduleRef = await Test.createTestingModule({ imports: [spokeEvents()] }).compile();

    const processor = moduleRef.get<BlockProcessor>(SPOKE_EVENT_PROCESSOR);

    expect(processor).toBeInstanceOf(AaveEventProcessor);
    // The name carries the address, so a retry reason says which Spoke stalled.
    expect(processor.name).toContain(SPOKE.slice(0, 10));
  });

  it('owns the seams the processor needs, so an importer supplies none', async () => {
    const moduleRef = await Test.createTestingModule({ imports: [spokeEvents()] }).compile();

    // A module in IndexingModule's `imports` cannot reach the LOG_READER that
    // module provides — exports flow outward, not inward — so this one binds
    // its own. Without that the processor could not live here at all.
    expect(moduleRef.get<LogReader>(LOG_READER)).toBeDefined();
    expect(moduleRef.get<EventStore>(SPOKE_EVENT_STORE)).toBeDefined();
  });

  it('re-exports ClickHouse, so a probe needs no second client', async () => {
    const moduleRef = await Test.createTestingModule({ imports: [spokeEvents()] }).compile();

    expect(moduleRef.get(ClickHouseHealthIndicator)).toBeInstanceOf(ClickHouseHealthIndicator);
  });

  it('takes a token, so a second Spoke is one more registration', async () => {
    const second = Symbol('SECOND_SPOKE_PROCESSOR');

    const moduleRef = await Test.createTestingModule({
      imports: [spokeEvents(SPOKE), spokeEvents(OTHER_SPOKE, second)],
    }).compile();

    // Both under one importer: without a distinct token the second export would
    // overwrite the first, and one of the two Spokes would silently go
    // unindexed.
    expect(moduleRef.get<BlockProcessor>(SPOKE_EVENT_PROCESSOR).name).toContain(SPOKE.slice(0, 10));
    expect(moduleRef.get<BlockProcessor>(second).name).toContain(OTHER_SPOKE.slice(0, 10));
  });

  it('reaches neither the node nor the database while building', async () => {
    const fetch = vi.fn<() => Promise<Response>>();
    vi.stubGlobal('fetch', fetch);

    const moduleRef = await Test.createTestingModule({ imports: [spokeEvents()] }).compile();
    moduleRef.get<BlockProcessor>(SPOKE_EVENT_PROCESSOR);
    moduleRef.get(CLICKHOUSE_CLIENT);

    // Boot must not depend on either being reachable — otherwise an outage in
    // one turns into a crash loop instead of a pod reporting not-ready.
    expect(fetch).not.toHaveBeenCalled();
  });
});
