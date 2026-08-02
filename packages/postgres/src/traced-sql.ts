import { SpanKind, SpanStatusCode, trace, type Span, type Tracer } from '@opentelemetry/api';
import type { Sql } from 'postgres';

/**
 * Wraps the postgres.js client so every query it runs becomes a span.
 *
 * **Why a `Proxy` and not an instrumentation.**
 * `@opentelemetry/instrumentation-pg` patches `pg` and `pg-pool`; this repo
 * uses postgres.js, which is a different driver with a different wire
 * implementation, so that package would install two require hooks and
 * instrument nothing. Nothing upstream instruments postgres.js.
 *
 * **Why not the `debug` hook postgres.js already offers.** It fires once, at
 * *send* time, and never on settle — so it cannot measure a duration, which is
 * the only thing worth having here.
 *
 * **Why here and not in the stores.** Nine query sites across five adapters
 * against one factory. The factory is the chokepoint, and wrapping the value
 * behind `POSTGRES_CLIENT` leaves every store injecting exactly what it did
 * before.
 *
 * Three things this cannot see, stated so nobody assumes otherwise:
 *
 * - **`.cursor()` and `.forEach()`** call postgres.js's `handle()` directly
 *   rather than going through `then`, so they are untraced. Nothing in the repo
 *   uses either.
 * - **`sql.begin(tx => …)`** hands the callback a *different* `Sql` built
 *   inside postgres.js, which this proxy never sees — so the migration runner's
 *   whole transaction is untraced. Acceptable: it is a one-shot job that
 *   already reports per database.
 * - **Values.** `db.query.text` is built from `strings.raw`, i.e. the template's
 *   literal parts with the interpolations left as `?`. Deliberate, and not
 *   merely convenient: a parameter can then never reach a span attribute, and
 *   it is also the form that groups in a trace backend.
 */
export function tracedSql(sql: Sql): Sql {
  const tracer = trace.getTracer('@packages/postgres');

  return new Proxy(sql, {
    apply(target, thisArg, args: unknown[]): unknown {
      const result: unknown = Reflect.apply(target, thisArg, args);

      // `sql` is overloaded. `` sql`SELECT …` `` is a query; `sql(values, ...cols)`
      // builds an insert fragment and `sql('name')` an identifier — both called
      // *inside* a template literal, and neither reaches the server on its own.
      // postgres.js discriminates them exactly this way, and
      // `postgres-token-metadata-store.ts` uses both forms.
      const first = args[0];
      if (!isTemplateStrings(first)) return result;

      return traceQuery(tracer, result, first.raw.join('?'));
    },
  });
}

interface TemplateStrings {
  readonly raw: readonly string[];
}

function isTemplateStrings(value: unknown): value is TemplateStrings {
  if (typeof value !== 'object' || value === null || !('raw' in value)) return false;
  return Array.isArray(value.raw);
}

/** The only part of postgres.js's `Query` this touches: it is a `PromiseLike`. */
type Thenable = PromiseLike<unknown>;

function isThenable(value: unknown): value is Thenable {
  if (typeof value !== 'object' || value === null || !('then' in value)) return false;
  return typeof value.then === 'function';
}

/**
 * Starts the span when the query is executed, not when it is built.
 *
 * postgres.js's `Query` extends `Promise` but is **lazy** — it runs on the
 * first `then`/`catch`/`finally`. Subscribing eagerly here would execute it at
 * construction, which every current call site would survive (they all await
 * immediately) and a future one would not. So `then` is shadowed instead: the
 * span opens on the first call, and the inherited `then` is what settles it.
 *
 * `catch` and `finally` need no shadow of their own — postgres.js delegates
 * both to `Promise.prototype`, which routes through `this.then`, which is this.
 *
 * Calling the inherited `then` twice — once for the span, once for the caller —
 * does not run the query twice: postgres.js guards execution behind an
 * `executed` flag. Measured, because the alternative was assuming it, and a
 * spec pins it by counting rows after a single tapped `INSERT`.
 */
function traceQuery(tracer: Tracer, query: unknown, statement: string): unknown {
  if (!isThenable(query)) return query;

  // Captured — and bound — before the shadow is installed, so this is the
  // inherited implementation rather than the one defined below.
  const inherited = query.then.bind(query);
  let span: Span | null = null;

  // oxlint-disable-next-line no-thenable
  Object.defineProperty(query, 'then', {
    configurable: true,
    writable: true,
    value(
      onFulfilled?: ((value: unknown) => unknown) | null,
      onRejected?: ((reason: unknown) => unknown) | null,
    ): Thenable {
      if (span === null) {
        span = tracer.startSpan(spanName(statement), {
          kind: SpanKind.CLIENT,
          attributes: {
            'db.system.name': 'postgresql',
            'db.query.text': statement,
            'db.operation.name': operationOf(statement),
          },
        });

        const started = span;
        inherited(
          () => started.end(),
          (error: unknown) => {
            if (error instanceof Error) started.recordException(error);
            started.setStatus({
              code: SpanStatusCode.ERROR,
              message: error instanceof Error ? error.message : String(error),
            });
            started.end();
          },
        );
      }

      return inherited(onFulfilled, onRejected);
    },
  });

  return query;
}

function operationOf(statement: string): string {
  return statement.trimStart().split(/\s/)[0]?.toUpperCase() ?? 'QUERY';
}

/**
 * Semantic conventions want `"{operation} {target}"`. The target is only taken
 * when it is unambiguous — a bad guess in a span name is worse than no target,
 * because the name is what someone groups by.
 */
function spanName(statement: string): string {
  const operation = operationOf(statement);
  const target = /\b(?:from|into|update|table)\s+([a-z_][a-z0-9_.]*)/i.exec(statement)?.[1];
  return target === undefined ? operation : `${operation} ${target}`;
}
