import 'reflect-metadata';

import { ConfigService } from '@nestjs/config';
import { NestFactory } from '@nestjs/core';
import { Logger } from 'nestjs-pino';

import { AppModule } from './app.module';
import type { Env } from './config/env';
import { installGracefulShutdown } from './lifecycle/graceful-shutdown';

async function bootstrap(): Promise<void> {
  const app = await NestFactory.create(AppModule, { bufferLogs: true });

  // Replace Nest's default logger before anything is flushed, so even boot
  // messages come out as structured JSON.
  app.useLogger(app.get(Logger));
  app.flushLogs();

  const config = app.get<ConfigService<Env, true>>(ConfigService);

  app.setGlobalPrefix(config.get('API_GLOBAL_PREFIX', { infer: true }), {
    exclude: ['health/live', 'health/ready'],
  });

  installGracefulShutdown(app, {
    graceSeconds: config.get('SHUTDOWN_GRACE_SECONDS', { infer: true }),
  });

  const port = config.get('API_PORT', { infer: true });
  const host = config.get('API_HOST', { infer: true });
  await app.listen(port, host);

  app.get(Logger).log(`api listening on http://${host}:${port}`, 'Bootstrap');
}

void bootstrap();
