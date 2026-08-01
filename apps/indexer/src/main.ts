import 'reflect-metadata';

import { ConfigService } from '@nestjs/config';
import { NestFactory } from '@nestjs/core';

import { AppModule } from './app.module';
import type { Env } from './config/env';
import { Logger, installGracefulShutdown } from '@packages/ops';

async function bootstrap(): Promise<void> {
  const app = await NestFactory.create(AppModule, { bufferLogs: true });

  // Replace Nest's default logger before anything is flushed, so even boot
  // messages come out as structured JSON.
  app.useLogger(app.get(Logger));
  app.flushLogs();

  const config = app.get<ConfigService<Env, true>>(ConfigService);

  installGracefulShutdown(app, {
    graceSeconds: config.get('SHUTDOWN_GRACE_SECONDS', { infer: true }),
  });

  // The worker listens purely so Kubernetes has a probe target; only
  // /health/live and /health/ready are mounted.
  const port = config.get('INDEXER_PORT', { infer: true });
  const host = config.get('INDEXER_HOST', { infer: true });
  await app.listen(port, host);

  app.get(Logger).log(`indexer probes listening on http://${host}:${port}`, 'Bootstrap');
}

void bootstrap();
