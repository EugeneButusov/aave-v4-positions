import 'reflect-metadata';

import { ConfigService } from '@nestjs/config';
import { NestFactory } from '@nestjs/core';

import { AppModule } from './app.module';
import type { Env } from './config/env';
import { Logger, installGracefulShutdown } from '@aave-v4-positions/ops';
import { setupOpenApi } from './openapi/openapi';

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

  // Registered after the global prefix so the document reports the paths the
  // service actually serves, and before listen() so the UI is up with the port.
  const docsPath = config.get('API_DOCS_PATH', { infer: true });
  setupOpenApi(app, { path: docsPath });

  installGracefulShutdown(app, {
    graceSeconds: config.get('SHUTDOWN_GRACE_SECONDS', { infer: true }),
  });

  const port = config.get('API_PORT', { infer: true });
  const host = config.get('API_HOST', { infer: true });
  await app.listen(port, host);

  const logger = app.get(Logger);
  logger.log(`api listening on http://${host}:${port}`, 'Bootstrap');
  logger.log(`openapi docs at http://${host}:${port}/${docsPath}`, 'Bootstrap');
}

void bootstrap();
