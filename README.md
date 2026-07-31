# aave-v4-positions

An indexer and read API for **Aave v4 user positions** on the Hub-and-Spoke architecture.

The protocol groundwork is in [`docs/aave-v4-protocol-analysis.md`](docs/aave-v4-protocol-analysis.md):
where position state actually lives in v4, which events reconstruct it, the share/asset maths, the
health-factor and price layers, and the ingestion constraints. Everything in this repository is built
against those findings rather than v3 intuition.

## Status

This is the **scaffold**. It stands up the workspace, both services, the test and lint toolchain, and
the operational shape a Kubernetes deployment expects.

**Present:** pnpm workspace, two runnable NestJS services, validated configuration, structured
logging, liveness/readiness probes, graceful drain, OpenAPI docs, Vitest, oxlint + Prettier,
lefthook.

**Not yet:** database and migrations, the ingestion pipeline itself, the enrichment source, the
positions endpoints, Docker Compose, CI. See [Not here yet](#not-here-yet).

## Layout

```
.
├── apps/
│   ├── api/                 read API — serves indexed and enriched positions
│   └── indexer/             worker — ingests Spoke/Hub events and folds them into state
├── docs/
│   └── aave-v4-protocol-analysis.md
├── packages/
│   └── platform/            cross-service Nest infrastructure (probes, logging, shutdown)
├── pnpm-workspace.yaml      workspace globs + dependency catalog
├── tsconfig.base.json       one strict compiler configuration, inherited everywhere
├── lefthook.yml             git hooks
└── vitest.config.mts        aggregates every app into one test run
```

Both services are NestJS applications. The API serves HTTP; the indexer is a worker that also
listens, purely so Kubernetes has a probe target — no business endpoints are mounted on it.

`packages/platform` holds what both services use identically — the probes, readiness indicators,
structured logging and shutdown sequence. It carries no Aave domain logic; it would look the same in
any Kubernetes-deployed Nest service, and wrapping `nestjs-pino` there means no app depends on it
directly.

`pnpm -r` walks the workspace in topological order, so the package builds before its consumers with
no extra wiring. Two consumers deliberately read its **source** instead of `dist`: each app's vitest
config aliases it, so tests can never exercise a stale build. `pnpm typecheck` is the exception — it
builds the package first and checks against the emitted `.d.ts`, which is what actually verifies the
surface consumers see.

## Prerequisites

- **Node 22.12+** — enforced by `engines` in the root `package.json` with `engine-strict` on, so an
  older runtime fails at install rather than at runtime. Developed on 24.
- **pnpm 11** — `corepack enable` picks up the `packageManager` field automatically.

## Getting started

```bash
pnpm install
```

That also installs the git hooks (via the `prepare` script). Then give each service an environment:

```bash
cp apps/api/.env.example apps/api/.env && cp apps/indexer/.env.example apps/indexer/.env
```

Run either service in watch mode:

```bash
pnpm dev:api
```

```bash
pnpm dev:indexer
```

The API listens on `:3000`, the indexer on `:3001`. Check them:

```bash
curl -s localhost:3000/health/ready && curl -s localhost:3001/health/ready
```

Browse the API contract at **<http://localhost:3000/docs>**.

## Commands

| command                             | what it does                                     |
| ----------------------------------- | ------------------------------------------------ |
| `pnpm build`                        | compile both services to `apps/*/dist`           |
| `pnpm dev:api` / `pnpm dev:indexer` | watch mode                                       |
| `pnpm test`                         | every app's tests in one run                     |
| `pnpm test:watch` / `pnpm test:cov` | watch mode / V8 coverage                         |
| `pnpm typecheck`                    | `tsc --noEmit` across the workspace              |
| `pnpm lint` / `pnpm lint:fix`       | oxlint, type-aware                               |
| `pnpm format` / `pnpm format:check` | Prettier                                         |
| `pnpm check`                        | format, lint, typecheck, test — what CI will run |
| `pnpm clean`                        | remove build output                              |

Scope anything to one service with `pnpm --filter @aave-v4-positions/api <script>`.

## Configuration

Every variable is parsed by a Zod schema at boot. An invalid value **aborts the process** rather than
defaulting silently — a pod that crash-loops on bad config is far easier to diagnose than one quietly
indexing the wrong chain. Deployed environments inject variables directly; the `.env` file is a
local-development convenience and is skipped entirely under `NODE_ENV=test`.

**Shared** — `NODE_ENV`, `LOG_LEVEL`, `LOG_PRETTY`, `SHUTDOWN_GRACE_SECONDS`.

**API** — `API_HOST`, `API_PORT` (3000), `API_GLOBAL_PREFIX` (`api`), `API_DOCS_PATH` (`docs`).

**Indexer** — `INDEXER_HOST`, `INDEXER_PORT` (3001), `CHAIN_ID` (1), and `RPC_URL`, the one variable
with no default. Ingestion reads logs only, so a **full node is sufficient**; historical _state_ is
what would need an archive node, and nothing on the read path calls `eth_call` (analysis §8). Note
that some public endpoints gate log history behind a paid tier.

See each service's `.env.example` for the annotated list.

## API documentation

Swagger UI is at `/docs`, with the raw document at `/docs/openapi.json` and `/docs/openapi.yaml`.
Like the probes, it sits outside the global API prefix — operational surface rather than part of the
versioned API. It is always served: the docs are the API's contract, not a debug affordance, and a
contract that is absent in the environment people actually call is not much of a contract.

`info.version` is the **API contract version**, deliberately not the package version. A dependency
bump changes the package and must not imply anything about the shape of the responses.

Two decisions worth knowing about before adding endpoints:

**Response types are decorated by hand, not by the `@nestjs/swagger` CLI plugin.** The plugin infers
`@ApiProperty` from TypeScript types, but it only runs through the Nest CLI build — under Vitest the
generated document would silently lose properties, so the contract test would be asserting something
the running service does not produce. Explicit decorators keep the two identical.

**A drift guard is part of the test suite.** A route added without `@ApiOkResponse` still routes and
still appears in the document, but with an empty response and no schema — the contract quietly stops
describing what the service returns, and nothing else fails.
[`openapi.e2e-spec.ts`](apps/api/test/openapi.e2e-spec.ts) walks every operation and fails if any
lacks a typed success response.

**Request validation is still an open choice.** There are no request DTOs yet. Configuration uses
Zod, so `nestjs-zod` (one schema driving both validation and the OpenAPI document) is the coherent
option; `class-validator` with `@ApiProperty` is the more conventional one. Worth deciding with the
first real endpoint rather than now.

## Operational shape

The pieces that exist because this is meant to run in Kubernetes, not on a laptop:

**Probes are split and mean different things.** `/health/live` reports only whether the process is
wedged and never touches a dependency — a database outage should drain traffic, not have kubelet
restart every replica. `/health/ready` aggregates registered dependency checks and answers `503` with
the failing check named in the body. Both sit outside the API's global prefix, so deployment
manifests do not move when the API is versioned.

**Readiness fails before the server closes.** `enableShutdownHooks()` closes the server the moment
SIGTERM lands, which races the endpoints controller — the pod can still be receiving traffic it is no
longer listening for. Instead
[`graceful-shutdown.ts`](packages/platform/src/lifecycle/graceful-shutdown.ts) fails readiness first, holds for
`SHUTDOWN_GRACE_SECONDS` so the removal propagates, and only then closes.

**Logs are one JSON object per line** (pino), with a request id propagated from `x-request-id` or
minted per request and echoed back on the response — that header is what makes a log line traceable
from a client-side report. Probe traffic is excluded so it does not bury everything else, and
`authorization`/`cookie` headers are removed rather than masked. Every line is tagged with the
service, plus the `chainId` on the indexer, so one stream can carry both. `LOG_PRETTY=true` is for
local use only. A service declares _what it is_, not how logging works:

```ts
LoggingModule.forRootAsync({
  imports: [ConfigModule],
  inject: [ConfigService],
  useFactory: (config: ConfigService<Env, true>) => ({
    service: 'api',
    level: config.get('LOG_LEVEL', { infer: true }),
    pretty: config.get('LOG_PRETTY', { infer: true }),
  }),
});
```

**Adding a dependency check is a provider binding.** Implement `HealthIndicator` from
`@aave-v4-positions/platform` and bind it to the `HEALTH_INDICATORS` token; readiness picks it up in
both services. The database check will be the first real one.

**Adding a data source is likewise a binding.** `EventSource` in
[`event-source.ts`](apps/indexer/src/ingestion/event-source.ts) is the seam — one source owns one
stream (a Spoke's position events, the Hub's asset events, a Chainlink aggregator's feed).
`IngestionService` starts each on bootstrap and aborts them on shutdown, so SIGTERM stops ingestion at
a known point rather than mid-batch; a source that throws is contained and logged rather than taking
the pod down with it. Nothing is registered yet, and the service says so at boot.

## Toolchain notes

Choices that are not the default, and why.

**oxlint + tsgolint instead of ESLint + typescript-eslint.** Measured, not assumed. Of the 115 rules
our ESLint config had enabled, oxlint implements 114 (the miss is `no-octal`), and the type-aware
ones run on a real TypeScript checker — `tsgolint` is a Go binary embedding `typescript-go`, which is
the compiler that became TS 7. The `no-unsafe-*` family and `no-floating-promises` report identically
to typescript-eslint, down to the character position. Whole repo: **~250ms versus ~2.2s**.

The `no-unsafe-*` family is the reason type-aware linting is not optional here. `tsc --noEmit`
does **not** catch `any` propagation — assigning an `any` is legal TypeScript, and `noImplicitAny`
governs declarations, not values flowing through them. A real instance of this shipped into this
repo: pino types `base` as `Record<string, any>`, and that contextual `any` silently swallowed a
`ConfigService.get` return. Only the linter saw it.

Two consequences of that choice:

- **`consistent-type-imports` is off.** No Rust linter is `emitDecoratorMetadata`-aware, so it
  rewrites Nest's constructor-injected imports to `import type` and erases them. Applying that
  "safe fix" produces `Nest can't resolve dependencies of the HealthController (?)` at runtime.
  typescript-eslint was the only linter of the three evaluated that got this right.
- **Biome was rejected.** It has no `no-unsafe-*` rules at all — it carries its own inference rather
  than a TypeScript checker, so that information does not exist for it to report on. On the pino bug
  above it is silent, and it makes the same DI-breaking `useImportType` call (its own docs recommend
  disabling that rule for NestJS).

**TypeScript 5.9 for now.** Nothing blocks 7 any more — the tsconfig is already free of the options
it removed, and TS 7 compiles this codebase clean. What holds it back is that `@nestjs/cli` pins
`typescript: 5.9.3` exactly, so bumping the root would leave `pnpm typecheck` on one compiler and
`nest build` on another. Worth one version, not two compilers.

**Vitest with no SWC step.** The usual Nest recipe adds `unplugin-swc` because esbuild does not emit
decorator metadata, which is exactly what Nest's injector reads. Vite's current transform _does_ emit
it, so the plugin and `@swc/core` are unnecessary — two fewer dependencies and roughly a 9× faster
transform. Because that is an implicit guarantee,
[`toolchain.spec.ts`](apps/api/src/toolchain.spec.ts) asserts `design:paramtypes` is present. If it
ever stops being emitted, that test fails first and says why, instead of every DI-backed test failing
at once with an opaque resolution error.

**Strict compiler settings**, including `noUncheckedIndexedAccess` and
`noPropertyAccessFromIndexSignature`. Financial arithmetic is the whole point of this service; an
implicit `any` on a balance is a silent wrong number.

**A pnpm catalog** holds every shared dependency version in `pnpm-workspace.yaml`, so the two services
cannot drift onto different Nest minors.

**Hooks are split by cost.** `pre-commit` runs oxlint and Prettier over staged files only; `pre-push`
runs the whole typecheck and test suite, since those are project-wide and too slow to attach to every
commit. Bypass with `LEFTHOOK=0`, or one job with `LEFTHOOK_EXCLUDE=<name>`.

## Not here yet

Deliberate, in rough order of what comes next.

- **Database, schema and migrations.** The analysis settles what has to be stored — immutable raw
  events plus a derived projection, which is also what makes reorg handling a replay rather than a
  patch (§8, §9).
- **The ingestion pipeline**: incremental sync, backfill, reorg handling, and the Hub asset mirror.
  That mirror is the highest-risk component — one mishandled transition silently corrupts every
  supply valuation for that asset, with no error, just wrong numbers.
- **The reconciliation job** designed in §9, which is what keeps the fold honest over time.
- **Enrichment** and the positions endpoints, per the §12 conclusion. Note that `uint256` amounts
  must be serialised as JSON strings — float64 has 53 bits of mantissa and share balances are far
  past it. The failure mode is a few wei of drift that reads as a rounding bug.
- **Docker Compose, CI, and Kubernetes manifests.** `pnpm check` is written to be exactly what CI
  runs.
