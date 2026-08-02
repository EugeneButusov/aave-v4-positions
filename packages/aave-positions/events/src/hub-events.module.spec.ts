import { ClickHouseHealthIndicator } from '@packages/clickhouse';
import { LOG_READER, type BlockProcessor, type LogReader } from '@packages/indexing';
import { Injectable, Module } from '@nestjs/common';
import { Test } from '@nestjs/testing';
import { describe, expect, it } from 'vitest';

import { AaveEventProcessor } from './aave-event-processor';
import { HUB_EVENT_STORE, type EventStore } from './store/event-store';
import { HUB_EVENT_PROCESSOR, HubEventsModule } from './hub-events.module';
import { SPOKE_EVENT_PROCESSOR, SpokeEventsModule } from './spoke-events.module';

const HUB = '0xcca852bc40e560adc3b1cc58ca5b55638ce826c9';
const SPOKE = '0x94e7a5dcbe816e498b89ab752661904e2f56c485';
const PLUS_HUB = '0x06002e9c4412cb7814a791ea3666d905871e536a';

@Injectable()
class Settings {
  readonly hub = HUB;
}

/** Stands in for the application's ConfigModule: options arrive through DI. */
@Module({ providers: [Settings], exports: [Settings] })
class SettingsModule {}

const clickhouse = {
  url: 'http://clickhouse.invalid',
  database: 'aave',
  username: 'aave',
  password: '',
};
const rpc = { rpcUrls: ['https://rpc.invalid'], rpcTimeoutMs: 1_000 };

function hubEvents(hub: string = HUB, token?: symbol) {
  return HubEventsModule.forRootAsync({
    imports: [SettingsModule],
    inject: [Settings],
    useFactory: () => ({ chainId: 1, hub, rpc, clickhouse }),
    ...(token && { token }),
  });
}

function spokeEvents() {
  return SpokeEventsModule.forRootAsync({
    useFactory: () => ({ chainId: 1, spoke: SPOKE, rpc, clickhouse }),
  });
}

describe('HubEventsModule', () => {
  it('exports a processor built for the configured Hub', async () => {
    const moduleRef = await Test.createTestingModule({ imports: [hubEvents()] }).compile();

    const processor = moduleRef.get<BlockProcessor>(HUB_EVENT_PROCESSOR);

    expect(processor).toBeInstanceOf(AaveEventProcessor);
    // The name carries the address, so a retry reason says which stream stalled.
    expect(processor.name).toContain(HUB.slice(0, 10));
  });

  it('owns the seams the processor needs, so an importer supplies none', async () => {
    const moduleRef = await Test.createTestingModule({ imports: [hubEvents()] }).compile();

    expect(moduleRef.get<LogReader>(LOG_READER)).toBeDefined();
    expect(moduleRef.get<EventStore>(HUB_EVENT_STORE)).toBeDefined();
  });

  it('re-exports ClickHouse, so a probe needs no second client', async () => {
    const moduleRef = await Test.createTestingModule({ imports: [hubEvents()] }).compile();

    expect(moduleRef.get(ClickHouseHealthIndicator)).toBeInstanceOf(ClickHouseHealthIndicator);
  });

  it('takes a token, so a second Hub is one more registration', async () => {
    const second = Symbol('PLUS_HUB_PROCESSOR');
    const moduleRef = await Test.createTestingModule({
      imports: [hubEvents(), hubEvents(PLUS_HUB, second)],
    }).compile();

    // Two Hubs, two tokens. Without the override both would export
    // HUB_EVENT_PROCESSOR into one importer and collide.
    expect(moduleRef.get<BlockProcessor>(HUB_EVENT_PROCESSOR).name).toContain(HUB.slice(0, 10));
    expect(moduleRef.get<BlockProcessor>(second).name).toContain(PLUS_HUB.slice(0, 10));
  });

  it('loads beside the Spoke module, each with its own processor', async () => {
    const moduleRef = await Test.createTestingModule({
      imports: [spokeEvents(), hubEvents()],
    }).compile();

    // Both are imported into one graph in the indexer. Distinct tokens and
    // distinct names are what let the loop dispatch a range to both and let a
    // failure say which one it was.
    const spoke = moduleRef.get<BlockProcessor>(SPOKE_EVENT_PROCESSOR);
    const hub = moduleRef.get<BlockProcessor>(HUB_EVENT_PROCESSOR);

    expect(spoke.name).toMatch(/^aave-spoke\(/);
    expect(hub.name).toMatch(/^aave-hub\(/);
  });
});
