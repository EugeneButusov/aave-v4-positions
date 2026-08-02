import { Module, type DynamicModule } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import { PositionsModule } from '@aave-positions/positions';
import { PostgresSyncStatusStore, SYNC_STATUS_STORE } from '@packages/indexing';
import { PostgresModule } from '@packages/postgres';

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
 * **Two databases, because the answer needs both.** ClickHouse holds the fold;
 * Postgres holds the indexer's cursor, which is how a response says which block
 * it is true as of. The API reads that row and writes nothing to it — going
 * through `SyncStatusStore` rather than issuing the SQL here keeps the table
 * owned by the package that migrates it, so a schema change breaks a typed port
 * instead of a string literal in an app.
 *
 * Both database modules are re-exported, so the importing module can register
 * their health indicators without constructing a second client of either. The
 * indicators resolve inside `HealthModule`'s own injector, which is why passing
 * the classes there is not enough on its own.
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

    const postgres = PostgresModule.forRootAsync({
      imports: [ConfigModule],
      inject: [ConfigService],
      useFactory: (config: ConfigService<Env, true>) => ({
        url: config.get('POSTGRES_URL', { infer: true }),
      }),
    });

    return {
      module: PositionsApiModule,
      imports: [ConfigModule, positions, postgres],
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
      exports: [positions, postgres],
    };
  }
}
