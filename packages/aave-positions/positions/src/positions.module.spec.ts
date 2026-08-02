import { CLICKHOUSE_CLIENT, ClickHouseHealthIndicator } from '@packages/clickhouse';
import { Injectable, Module } from '@nestjs/common';
import { Test } from '@nestjs/testing';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ClickHousePositionStore } from './store/clickhouse-position-store';
import { POSITION_STORE, type PositionStore } from './store/position-store';
import { PositionsModule } from './positions.module';

@Injectable()
class Settings {
  readonly url = 'http://clickhouse.invalid';
}

/** Stands in for the application's ConfigModule: options arrive through DI. */
@Module({ providers: [Settings], exports: [Settings] })
class SettingsModule {}

const positions = () =>
  PositionsModule.forRootAsync({
    imports: [SettingsModule],
    inject: [Settings],
    useFactory: (settings: Settings) => ({
      clickhouse: { url: settings.url, database: 'aave', username: 'aave', password: '' },
      cursorSecret: 'spec-cursor-secret'.padEnd(32, '.'),
    }),
  });

describe('PositionsModule', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('exports the store', async () => {
    const moduleRef = await Test.createTestingModule({ imports: [positions()] }).compile();

    expect(moduleRef.get<PositionStore>(POSITION_STORE)).toBeInstanceOf(ClickHousePositionStore);
  });

  it('re-exports ClickHouse, so a probe needs no second client', async () => {
    const moduleRef = await Test.createTestingModule({ imports: [positions()] }).compile();

    expect(moduleRef.get(ClickHouseHealthIndicator)).toBeInstanceOf(ClickHouseHealthIndicator);
  });

  it('reaches the database not at all while building', async () => {
    const fetch = vi.fn<() => Promise<Response>>();
    vi.stubGlobal('fetch', fetch);

    const moduleRef = await Test.createTestingModule({ imports: [positions()] }).compile();
    moduleRef.get<PositionStore>(POSITION_STORE);
    moduleRef.get(CLICKHOUSE_CLIENT);

    // Boot must not depend on the database being reachable, or an outage turns
    // into a crash loop instead of a pod reporting not-ready.
    expect(fetch).not.toHaveBeenCalled();
  });
});
