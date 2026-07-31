import { randomUUID } from 'node:crypto';
import type { IncomingMessage, ServerResponse } from 'node:http';

import { Module, RequestMethod } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import { LoggerModule as PinoLoggerModule } from 'nestjs-pino';

import type { Env } from '../config/env';

const REQUEST_ID_HEADER = 'x-request-id';

/**
 * Structured logging for the whole process: one JSON object per line. The HTTP
 * side only ever sees probe traffic, but the same logger instance is what the
 * ingestion services write to.
 */
@Module({
  imports: [
    PinoLoggerModule.forRootAsync({
      imports: [ConfigModule],
      inject: [ConfigService],
      useFactory: (config: ConfigService<Env, true>) => {
        // Read into locals first: pino types `base` as `Record<string, any>`,
        // and that contextual `any` would otherwise swallow the config types.
        const level = config.get('LOG_LEVEL', { infer: true });
        const pretty = config.get('LOG_PRETTY', { infer: true });
        const chainId = config.get('CHAIN_ID', { infer: true });

        return {
          // Express 5 rejects the bare `*` that nestjs-pino defaults to; the
          // named-wildcard form is the path-to-regexp v8 spelling of the same thing.
          forRoutes: [{ path: '{*path}', method: RequestMethod.ALL }],
          pinoHttp: {
            level,
            transport: pretty
              ? {
                  target: 'pino-pretty',
                  options: { singleLine: true, translateTime: 'SYS:HH:MM:ss.l' },
                }
              : undefined,
            base: { service: 'indexer', chainId },
            genReqId: (req: IncomingMessage, res: ServerResponse) => {
              const incoming = req.headers[REQUEST_ID_HEADER];
              const id = (Array.isArray(incoming) ? incoming[0] : incoming) ?? randomUUID();
              res.setHeader(REQUEST_ID_HEADER, id);
              return id;
            },
            // Probes fire every few seconds; logging them buries everything else.
            autoLogging: {
              ignore: (req: IncomingMessage) => req.url?.startsWith('/health') ?? false,
            },
            redact: {
              // RPC endpoints routinely carry an API key in the path.
              paths: ['req.headers.authorization', 'req.headers.cookie'],
              remove: true,
            },
          },
        };
      },
    }),
  ],
})
export class LoggingModule {}
