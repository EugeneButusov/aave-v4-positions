import { Module, type DynamicModule, type ModuleMetadata } from '@nestjs/common';
import { LoggerModule as PinoLoggerModule } from 'nestjs-pino';

import { buildLoggingParams, type LoggingOptions } from './logging.options';

/**
 * `TDeps` is inferred from the factory's own parameter list, which is what lets
 * the injected dependencies stay typed end to end — Nest's usual `any[]`
 * factory signature would erase them.
 */
export interface LoggingAsyncOptions<TDeps extends unknown[] = unknown[]> extends Pick<
  ModuleMetadata,
  'imports'
> {
  inject?: NonNullable<ModuleMetadata['providers']>;
  useFactory: (...args: TDeps) => LoggingOptions | Promise<LoggingOptions>;
}

/**
 * Structured logging: one JSON object per line, for every service.
 *
 * Wrapping nestjs-pino rather than re-exporting it keeps the pino
 * configuration in one place — services declare *what* they are, not *how*
 * logging is set up — and means no app has to depend on nestjs-pino directly.
 */
@Module({})
export class LoggingModule {
  static forRootAsync<TDeps extends unknown[]>(options: LoggingAsyncOptions<TDeps>): DynamicModule {
    return {
      module: LoggingModule,
      imports: [
        PinoLoggerModule.forRootAsync({
          imports: options.imports,
          inject: options.inject,
          useFactory: async (...args: TDeps) =>
            buildLoggingParams(await options.useFactory(...args)),
        }),
      ],
      exports: [PinoLoggerModule],
    };
  }
}
