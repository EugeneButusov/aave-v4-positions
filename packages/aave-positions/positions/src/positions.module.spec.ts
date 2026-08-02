import { CLICKHOUSE_CLIENT, ClickHouseHealthIndicator } from '@packages/clickhouse';
import { Injectable, Module } from '@nestjs/common';
import { Test } from '@nestjs/testing';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ClickHousePositionStore } from './store/clickhouse-position-store';
import { ClickHouseHubAssetStore } from './store/clickhouse-hub-asset-store';
import { HUB_ASSET_STORE, type HubAssetStore } from './store/hub-asset-store';
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

  it('exports the Hub asset store beside the position one', async () => {
    const moduleRef = await Test.createTestingModule({ imports: [positions()] }).compile();

    // Both or neither: nothing values a position without the Hub's index, so a
    // graph that hands out one and not the other only makes incomplete wiring
    // possible.
    expect(moduleRef.get<HubAssetStore>(HUB_ASSET_STORE)).toBeInstanceOf(ClickHouseHubAssetStore);
  });

  it('re-exports ClickHouse, so a probe needs no second client', async () => {
    const moduleRef = await Test.createTestingModule({ imports: [positions()] }).compile();

    expect(moduleRef.get(ClickHouseHealthIndicator)).toBeInstanceOf(ClickHouseHealthIndicator);
  });

  it('takes options with no imports at all', async () => {
    // A factory that closes over its configuration rather than injecting it —
    // which is every test harness, and any application that reads its settings
    // before Nest starts.
    const moduleRef = await Test.createTestingModule({
      imports: [
        PositionsModule.forRootAsync({
          useFactory: () => ({
            clickhouse: {
              url: 'http://clickhouse.invalid',
              database: 'aave',
              username: 'aave',
              password: '',
            },
            cursorSecret: 'spec-cursor-secret'.padEnd(32, '.'),
          }),
        }),
      ],
    }).compile();

    expect(moduleRef.get<PositionStore>(POSITION_STORE)).toBeInstanceOf(ClickHousePositionStore);
  });

  it('rejects a cursor secret short enough to guess', async () => {
    await expect(
      Test.createTestingModule({
        imports: [
          PositionsModule.forRootAsync({
            useFactory: () => ({
              clickhouse: {
                url: 'http://clickhouse.invalid',
                database: 'aave',
                username: 'aave',
                password: '',
              },
              cursorSecret: 'too-short',
            }),
          }),
        ],
      }).compile(),
    ).rejects.toThrow(/at least 32/);
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
