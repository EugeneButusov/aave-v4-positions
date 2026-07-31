import { Module } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import { HealthModule, LoggingModule } from '@aave-v4-positions/ops';

import { validateEnv, type Env } from './config/env';
import { HelloModule } from './hello/hello.module';

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
    HealthModule,
    HelloModule,
  ],
})
export class AppModule {}
