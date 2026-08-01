import { Module } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import { LoggingModule } from '@packages/ops';

import { validateEnv, type Env } from '../config/env';
import { indexingSetup } from '../indexing.setup';

/** The events module comes in through `indexing`, which already imports it. */
const { indexing } = indexingSetup({ autoStart: false });

/**
 * The backfill command's root: `AppModule` without the probe server, since
 * nothing polls a process that exits, and with the loop switched off.
 *
 * `autoStart: false` is the part that matters. Without it, loading this wiring
 * would start a full indexer alongside the backfill and race it up the chain.
 */
@Module({
  imports: [
    ConfigModule.forRoot({
      isGlobal: true,
      cache: true,
      envFilePath: ['.env'],
      ignoreEnvFile: process.env['NODE_ENV'] === 'test',
      validate: validateEnv,
    }),
    LoggingModule.forRootAsync({
      imports: [ConfigModule],
      inject: [ConfigService],
      useFactory: (config: ConfigService<Env, true>) => ({
        // Tagged apart from the daemon's, so one log stream can carry both and
        // still answer "was this the loop or a backfill?".
        service: 'indexer-backfill',
        level: config.get('LOG_LEVEL', { infer: true }),
        pretty: config.get('LOG_PRETTY', { infer: true }),
        base: { chainId: config.get('CHAIN_ID', { infer: true }) },
      }),
    }),
    indexing,
  ],
})
export class BackfillModule {}
