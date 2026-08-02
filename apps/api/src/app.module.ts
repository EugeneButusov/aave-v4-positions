import { Module } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import { ClickHouseHealthIndicator } from '@packages/clickhouse';
import { HealthModule, LoggingModule } from '@packages/ops';
import { PostgresHealthIndicator } from '@packages/postgres';

import { validateEnv, type Env } from './config/env';
import { PositionsApiModule } from './positions/positions-api.module';

// Built once and referenced twice: Nest identifies a dynamic module by object
// reference, so a second `forRoot()` call would build a second pair of database
// clients and the health probes would be checking neither of the ones serving
// requests.
const positions = PositionsApiModule.forRoot();

@Module({
  imports: [
    ConfigModule.forRoot({
      isGlobal: true,
      cache: true,
      // Deployed environments inject variables directly; the dotenv file is a
      // local-development convenience only, and tests stay hermetic.
      envFilePath: ['.env'],
      ignoreEnvFile: process.env['NODE_ENV'] === 'test',
      validate: validateEnv,
    }),
    LoggingModule.forRootAsync({
      imports: [ConfigModule],
      inject: [ConfigService],
      useFactory: (config: ConfigService<Env, true>) => ({
        service: 'api',
        level: config.get('LOG_LEVEL', { infer: true }),
        pretty: config.get('LOG_PRETTY', { infer: true }),
      }),
    }),
    positions,
    // The indicators resolve inside HealthModule's own injector, so they have to
    // arrive through a module it imports. PositionsApiModule re-exports both
    // database modules for exactly this.
    HealthModule.forRoot({
      imports: [positions],
      indicators: [ClickHouseHealthIndicator, PostgresHealthIndicator],
    }),
  ],
})
export class AppModule {}
