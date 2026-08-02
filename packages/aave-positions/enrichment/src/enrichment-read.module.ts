import {
  Module,
  type DynamicModule,
  type InjectionToken,
  type ModuleMetadata,
  type OptionalFactoryDependency,
} from '@nestjs/common';
import { PostgresModule, type PostgresOptions } from '@packages/postgres';

import { PostgresTokenMetadataStore } from './store/postgres-token-metadata-store';
import { TOKEN_METADATA_STORE } from './store/token-metadata-store';

const ENRICHMENT_READ_OPTIONS = Symbol('ENRICHMENT_READ_OPTIONS');

export interface EnrichmentReadOptions {
  readonly postgres: PostgresOptions;
}

export interface EnrichmentReadAsyncOptions<TDeps extends unknown[] = unknown[]> extends Pick<
  ModuleMetadata,
  'imports'
> {
  inject?: (InjectionToken | OptionalFactoryDependency)[];
  useFactory: (...args: TDeps) => EnrichmentReadOptions | Promise<EnrichmentReadOptions>;
}

/** Holds the resolved options so the database module can read them too. */
@Module({})
class EnrichmentReadOptionsModule {}

/**
 * The **read** side of enrichment: the stores, and no writer.
 *
 * Separate from {@link TokenEnrichmentModule} because the two have different
 * consumers and only one of them should ever run. That module owns the
 * processor that fills the table; an API replica must never acquire one by
 * importing the thing it reads, or every replica would fan out over the same
 * ERC-20s and race the same rows.
 *
 * **Postgres and nothing else**, which is the whole reason this package exists.
 * What it holds is fetched rather than folded, small, keyed for point lookups
 * and replaced in place; `010_token_metadata.sql` has the measurements that put
 * it here instead of in the column store beside the folds.
 *
 * It re-exports the database module, so an importer can register its health
 * indicator — and read the indexer's cursor — without constructing a second
 * client. `PositionsModule` used to be where the API took its Postgres from,
 * back when it owned a Postgres table; it no longer owns one, and this does.
 */
@Module({})
export class EnrichmentReadModule {
  static forRootAsync<TDeps extends unknown[]>(
    options: EnrichmentReadAsyncOptions<TDeps>,
  ): DynamicModule {
    const settings: DynamicModule = {
      module: EnrichmentReadOptionsModule,
      imports: options.imports ?? [],
      providers: [
        {
          provide: ENRICHMENT_READ_OPTIONS,
          useFactory: options.useFactory,
          inject: options.inject,
        },
      ],
      exports: [ENRICHMENT_READ_OPTIONS],
    };

    const postgres = PostgresModule.forRootAsync({
      imports: [settings],
      inject: [ENRICHMENT_READ_OPTIONS],
      useFactory: (resolved: EnrichmentReadOptions) => resolved.postgres,
    });

    return {
      module: EnrichmentReadModule,
      imports: [settings, postgres],
      providers: [{ provide: TOKEN_METADATA_STORE, useClass: PostgresTokenMetadataStore }],
      exports: [TOKEN_METADATA_STORE, postgres],
    };
  }
}
