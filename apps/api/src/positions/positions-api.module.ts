import { Module, type DynamicModule } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import { EnrichmentReadModule } from '@aave-positions/enrichment';
import { PositionsModule } from '@aave-positions/positions';
import { PostgresSyncStatusStore, SYNC_STATUS_STORE } from '@packages/indexing';

import type { Env } from '../config/env';
import { PositionCursors } from './position-cursors';
import { PositionsController } from './positions.controller';
import { PositionsService, STALE_AFTER_SECONDS } from './positions.service';

/**
 * The positions endpoint and the two databases behind it.
 *
 * Named apart from `@aave-positions/positions`' own `PositionsModule`, which it
 * builds: that one owns the query, this one owns the HTTP surface over it.
 *
 * **Two databases, because the answer needs both.** ClickHouse holds the fold.
 * Postgres holds the indexer's cursor — which is how a response says the block
 * it is true as of — and the token metadata the labels come from. The API reads
 * both and writes to neither; going through `SyncStatusStore` rather than
 * issuing the SQL here keeps the table owned by the package that migrates it,
 * so a schema change breaks a typed port instead of a string literal in an app.
 *
 * Both clients come from `PositionsModule`, which builds and re-exports them,
 * and are re-exported again so the importing module can register their health
 * indicators without constructing a third. The indicators resolve inside
 * `HealthModule`'s own injector, which is why passing the classes there is not
 * enough on its own.
 */
@Module({})
export class PositionsApiModule {
  static forRoot(): DynamicModule {
    const positions = PositionsModule.forRootAsync({
      imports: [ConfigModule],
      inject: [ConfigService],
      useFactory: (config: ConfigService<Env, true>) => ({
        clickhouse: {
          url: config.get('CLICKHOUSE_URL', { infer: true }),
          database: config.get('CLICKHOUSE_DATABASE', { infer: true }),
          username: config.get('CLICKHOUSE_USER', { infer: true }),
          password: config.get('CLICKHOUSE_PASSWORD', { infer: true }),
        },
      }),
    });

    // **The read side only.** The processor that fills this table lives in
    // `TokenEnrichmentModule`, and an API replica must never acquire one by
    // importing the thing it reads — every replica would then fan out over the
    // same ERC-20s and race the same rows.
    const enrichment = EnrichmentReadModule.forRootAsync({
      imports: [ConfigModule],
      inject: [ConfigService],
      useFactory: (config: ConfigService<Env, true>) => ({
        postgres: { url: config.get('POSTGRES_URL', { infer: true }) },
      }),
    });

    return {
      module: PositionsApiModule,
      imports: [ConfigModule, positions, enrichment],
      controllers: [PositionsController],
      providers: [
        PositionsService,
        { provide: SYNC_STATUS_STORE, useClass: PostgresSyncStatusStore },
        {
          // The key that signs a page token is this service's configuration,
          // and the store deals in page keys precisely so it never sees one.
          provide: PositionCursors,
          useFactory: (config: ConfigService<Env, true>) =>
            new PositionCursors(config.get('POSITIONS_CURSOR_SECRET', { infer: true })),
          inject: [ConfigService],
        },
        {
          provide: STALE_AFTER_SECONDS,
          useFactory: (config: ConfigService<Env, true>): number =>
            config.get('API_SYNC_STALE_AFTER_SECONDS', { infer: true }),
          inject: [ConfigService],
        },
      ],
      exports: [positions, enrichment],
    };
  }
}
