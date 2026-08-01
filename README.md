# aave-v4-positions

[![CI](https://github.com/EugeneButusov/aave-v4-positions/actions/workflows/ci.yml/badge.svg)](https://github.com/EugeneButusov/aave-v4-positions/actions/workflows/ci.yml)

An indexer and read API for **Aave v4 user positions** on the Hub-and-Spoke architecture.

The protocol groundwork is in [`docs/aave-v4-protocol-analysis.md`](docs/aave-v4-protocol-analysis.md):
where position state actually lives in v4, which events reconstruct it, the share/asset maths, the
health-factor and price layers, and the ingestion constraints. Everything in this repository is built
against those findings rather than v3 intuition.

## Status

The **scaffold plus the indexing framework**. The workspace, both services, the test and lint
toolchain, the operational shape a Kubernetes deployment expects — and a loop that walks the chain
and hands block ranges to injected processors.

**Present:** pnpm workspace, two runnable NestJS services, validated configuration, structured
logging, liveness/readiness probes, graceful drain, OpenAPI docs, Vitest, oxlint + Prettier,
lefthook, and the [indexing framework](#indexing) — chain client with provider failover, cursor,
reorg and processor seams.

**Not yet:** database and migrations, event decoding, real reorg detection, the enrichment source,
the positions endpoints, Kubernetes manifests. The API serves one stub endpoint; the indexer walks
the chain but its processors and reorg detector are placeholders. See
[Not here yet](#not-here-yet).

## Layout

```
.
├── apps/
│   ├── api/                 read API — indexed and enriched positions
│   └── indexer/             worker — Spoke/Hub event ingestion
│       └── src/
│           ├── chain/       RPC access — the ChainClient port and its viem adapter
│           └── indexing/    the loop, and the processor / reorg / cursor seams
├── docs/
│   └── aave-v4-protocol-analysis.md
├── packages/
│   └── ops/                 probes, logging, graceful shutdown — no domain logic
├── pnpm-workspace.yaml      workspace globs + dependency catalog
├── tsconfig.base.json       one strict compiler configuration, inherited everywhere
├── lefthook.yml             git hooks
└── vitest.config.mts        aggregates every workspace project into one test run
```

Both services are NestJS applications. The API serves HTTP; the indexer is a worker that also
listens, purely so Kubernetes has a probe target — no business endpoints are mounted on it.

`packages/ops` holds the operational concerns both services share: probes, structured logging and
the shutdown sequence — everything an operator needs and nothing a position needs. The name is the
boundary. It carries no Aave domain knowledge and would work in any Kubernetes-deployed Nest service,
so the share maths and the database layer become their own packages rather than accumulating here.
Wrapping `nestjs-pino` there also means no app depends on it directly.

`pnpm -r` walks the workspace in topological order, so the package builds before its consumers with
no extra wiring. Two consumers deliberately read its **source** instead of `dist`: each app's vitest
config aliases it, so tests can never exercise a stale build. `pnpm typecheck` is the exception — it
builds the package first and checks against the emitted `.d.ts`, which is what actually verifies the
surface consumers see.

## Prerequisites

- **Node 24** — enforced by `engines` with `engine-strict` on, so an older runtime fails at install
  rather than at runtime. It is the only version CI runs, so the declaration and the evidence match.
- **pnpm 11** — `corepack enable` picks up the `packageManager` field automatically.

There is no Nest CLI. `pnpm build` is `tsc`, and `pnpm dev:*` is `tsc --watch` plus `node --watch`.

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

## Running with Docker

Nothing to install but Docker — no Node, no pnpm:

```bash
docker compose up --build
```

Both services come up with health checks; `docker compose ps` shows them as `healthy` once their
readiness probes pass. Same addresses as above (`:3000`, `:3001`, `/docs`). Tear down with
`docker compose down`.

The indexer starts indexing immediately, against **`https://eth.drpc.org` by default** — so
`docker compose up` makes real requests to a third party. Point it somewhere else with
`RPC_URLS=https://your-node docker compose up`, or set `INDEXER_AUTOSTART=false` to bring up the
probes without indexing. Watching `docker compose logs -f indexer` is the quickest way to see the
framework work: it walks 1000-block ranges up from the Main Spoke genesis, and once it reaches the
tip it switches to one block at a time. Measured on a cold start: 938 ranges to cover the backfill,
with the final one self-truncating to 571 blocks so it stopped exactly on the finality boundary.

If 3000 or 3001 are taken:

```bash
API_PORT=4000 INDEXER_PORT=4001 docker compose up --build
```

One [`Dockerfile`](Dockerfile) serves both services — compose passes `APP=api` or `APP=indexer`. It
is a multi-stage build: manifests are copied before sources so the dependency layer survives a source
edit, and `pnpm deploy` collects the app, its built workspace packages and production-only
`node_modules` into a self-contained directory. The runtime stage carries that and nothing else, and
runs as the image's unprivileged `node` user.

Two details that exist so the drain actually works in a container:

- **`CMD` is exec form.** A shell wrapper would sit between Docker and Node and swallow `SIGTERM`,
  so the readiness-first shutdown would never run and the container would die on the timeout instead.
- **`stop_grace_period` (20s) exceeds `SHUTDOWN_GRACE_SECONDS` (5s).** Docker's default is 10s; if
  the grace window ever grew past it, `SIGKILL` would land mid-drain. Verified end to end — readiness
  answers `503` while the container is still serving, then it exits 0.

## Commands

| command                             | what it does                                 |
| ----------------------------------- | -------------------------------------------- |
| `pnpm build`                        | compile both services to `apps/*/dist`       |
| `pnpm dev:api` / `pnpm dev:indexer` | watch mode                                   |
| `pnpm test`                         | every app's tests in one run                 |
| `pnpm test:watch` / `pnpm test:cov` | watch mode / V8 coverage                     |
| `pnpm typecheck`                    | `tsc --noEmit` across the workspace          |
| `pnpm lint` / `pnpm lint:fix`       | oxlint, type-aware                           |
| `pnpm format` / `pnpm format:check` | Prettier                                     |
| `pnpm check`                        | format, lint, typecheck, test — what CI runs |
| `pnpm clean`                        | remove build output                          |

Scope anything to one service with `pnpm --filter @aave-v4-positions/api <script>`.

## Configuration

Every variable is parsed by a Zod schema at boot. An invalid value **aborts the process** rather than
defaulting silently — a pod that crash-loops on bad config is far easier to diagnose than one quietly
indexing the wrong chain. Deployed environments inject variables directly; the `.env` file is a
local-development convenience and is skipped entirely under `NODE_ENV=test`.

**Shared** — `NODE_ENV`, `LOG_LEVEL`, `LOG_PRETTY`, `SHUTDOWN_GRACE_SECONDS`.

**API** — `API_HOST`, `API_PORT` (3000), `API_GLOBAL_PREFIX` (`api`), `API_DOCS_PATH` (`docs`).

**Indexer** — `INDEXER_HOST`, `INDEXER_PORT` (3001), plus the chain configuration:

| variable                     | default    |                                                                    |
| ---------------------------- | ---------- | ------------------------------------------------------------------ |
| `CHAIN_ID`                   | _required_ | Checked against what the providers report, on the first iteration. |
| `RPC_URLS`                   | _required_ | Comma-separated, tried in order.                                   |
| `FINALITY_DEPTH`             | `128`      | Read by the reorg detector, never by the loop.                     |
| `INDEXER_START_BLOCK`        | `24720899` | Main Spoke genesis; used only when no cursor exists.               |
| `INDEXER_MAX_RANGE_SIZE`     | `1000`     | Blocks per dispatch while catching up.                             |
| `INDEXER_POLL_INTERVAL_MS`   | `4000`     |                                                                    |
| `INDEXER_RPC_TIMEOUT_MS`     | `10000`    |                                                                    |
| `INDEXER_STALL_THRESHOLD_MS` | `300000`   | How long without progress before readiness fails.                  |
| `INDEXER_AUTOSTART`          | `true`     | `false` boots the probes without indexing.                         |

`CHAIN_ID` and `RPC_URLS` deliberately have **no defaults**. A default chain id is precisely the
failure this validation exists to prevent: an indexer quietly pointed at the wrong chain produces
plausible, wrong data rather than an error.

A **full node is sufficient** — ingestion reads logs only. Historical _state_ is what would need an
archive node, and nothing on the read path calls `eth_call` (analysis §8). Provider capability does
vary, though: measured `eth_getLogs` ranges across public endpoints run from 50 blocks to 10,000, and
some serve no history at all, which is why the provider list is ordered and the range size adapts.

See each service's `.env.example` for the annotated list.

## API documentation

Swagger UI is at `/docs`, with the raw document at `/docs/openapi.json` and `/docs/openapi.yaml`.
Like the probes, it sits outside the global API prefix — operational surface rather than part of the
versioned API. It is always served: the docs are the API's contract, not a debug affordance, and a
contract that is absent in the environment people actually call is not much of a contract.

`info.version` is the **API contract version**, deliberately not the package version. A dependency
bump changes the package and must not imply anything about the shape of the responses.

Two decisions worth knowing about before adding endpoints:

**Response types are decorated by hand.** The `@nestjs/swagger` CLI plugin can infer `@ApiProperty`
from TypeScript types, but it only runs through the Nest CLI build — which this repo no longer uses,
and which never applied under Vitest anyway, so the generated document would have differed between
the test suite and the running service. Explicit decorators keep the two identical.

**A drift guard is part of the test suite.** A route added without `@ApiOkResponse` still routes and
still appears in the document, but with an empty response and no schema — the contract quietly stops
describing what the service returns, and nothing else fails.
[`openapi.e2e-spec.ts`](apps/api/test/openapi.e2e-spec.ts) walks every operation and fails if any
lacks a typed success response.

**Request validation is still an open choice.** There are no request DTOs yet. Configuration uses
Zod, so `nestjs-zod` (one schema driving both validation and the OpenAPI document) is the coherent
option; `class-validator` with `@ApiProperty` is the more conventional one. Worth deciding with the
first real endpoint rather than now.

## Indexing

The indexer walks the chain and hands block ranges to registered processors. The loop itself knows
about block numbers and nothing else — no notion of finality, of what a fork looks like, or of what
a processor does with a range. Each of those sits behind a port.

**Three seams, all injected at module setup.**

```ts
interface BlockProcessor {
  onBlockRange(from, to, signal): ProcessorOutcome | Promise<ProcessorOutcome>;
  onReorg(
    firstInvalidBlock,
    lastInvalidBlock,
    signal,
  ): ProcessorOutcome | Promise<ProcessorOutcome>;
}
interface ReorgDetector {
  bootstrap(cursor): Promise<ReorgVerdict>; // fill the window, vet the resume point
  safeHead(observedHead): number; // what is settled
  inspect(header): ReorgVerdict; // continuous | reorg | unrecoverable
  commit(header);
  rewindTo(lastValidBlock);
}
interface CursorStore {
  load(chainId);
  save(cursor);
}
```

**The detector owns the shape of the chain.** The loop never computes `head - finalityDepth`; it
calls `safeHead()` and branches on the answer. Blocks at or below it are settled, so they are
dispatched in wide ranges with no per-block inspection — which is what makes a ~932k-block backfill
take minutes. Above it, one block at a time, inspected first. That boundary is recomputed from a
fresh head every iteration rather than latched as a mode, and a range never straddles it. Swapping
the depth heuristic for `getBlock({ blockTag: 'finalized' })` later changes one file.

**Processors gate the cursor.** They return `ok` / `retry` / `failed` rather than throwing, and run
sequentially in registration order, stopping at the first non-`ok`. The cursor advances only when all
of them succeeded, and it is saved **last** — it is the single durable commit point, so a crash
anywhere earlier replays the range rather than skipping it. The corollary is that dispatch is
**at-least-once per processor per range**, and processors must be idempotent. That is not a wart: the
analysis already models positions as a fold over an immutable log, so replay is the repair primitive
(§8, §9.4).

A `retry` may set `narrowRange`, which halves the range size — the measured provider caps above are
why. Retries are unbounded, because an RPC outage should recover on its own; the bound is
observability, not control, and `/health/ready` fails once the loop has made no progress for
`INDEXER_STALL_THRESHOLD_MS`. `failed` is reserved for "more attempts cannot help" and is terminal.

**Resume is a detector question too.** On start the loop loads the cursor and hands it to
`bootstrap()`, which fills the retention window and reports whether that block is still canonical —
the process may have been stopped across a fork, and nothing else in the design would notice.

**Provider failover is viem's `fallback`**, tried in list order. Two of its defaults are overridden,
both verified against the 2.55 source rather than the docs: `fallback`'s own `retryCount` goes to 0
so a total outage surfaces immediately instead of after four full sweeps of the list, and the head is
clamped monotonically in the loop — viem does not reconcile block height between providers, so
failing over to a lagging node otherwise looks exactly like a reorg.

**What is real and what is not.** The chain client, the loop, the cursor seam and the outcome
protocol all work. `NoopReorgDetector` answers the finality question but detects nothing, and
`LoggingBlockProcessor` only logs. The reorg, bootstrap and failure paths are exercised by tests
through scripted fakes, not by the running service.

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
[`graceful-shutdown.ts`](packages/ops/src/lifecycle/graceful-shutdown.ts) fails readiness first, holds for
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

**Adding a dependency check is one line.** Implement `HealthIndicator` and register it; each is
resolved through DI, so an indicator can inject a connection or a client, and readiness aggregates
the results:

```ts
HealthModule.forRoot({
  imports: [indexing],
  indicators: [IndexerHealthIndicator],
});
```

That is the indexer's real registration, and the first use of the seam. It reports down on two
conditions only: the loop has stopped, or it has made no progress for longer than
`INDEXER_STALL_THRESHOLD_MS`. Before the first iteration it reports **up** — an indicator that fails
while starting would stop the pod ever becoming ready.

Worth being honest about what it buys. The indexer serves probes and nothing else, so failing
readiness drains no traffic, and compose's `restart: unless-stopped` does not react to an unhealthy
health check. It is an **alertable signal with no automatic recovery** — something above it has to
act. A `failed` loop stays failed until the process restarts, which is the same posture the rest of
the service takes towards bad configuration.

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

**TypeScript 7, and no Nest CLI.** TS 7 is the native compiler: typechecking this workspace drops
from ~620ms to ~180ms, and that gap widens with the codebase. It cost the Nest CLI — TS 7.0 ships
only the `tsc` executable, with no programmatic compiler API until 7.1, so `nest build` and
`nest start --watch` both refuse to run. Neither was earning its place. `nest build` was `tsc` plus
`deleteOutDir`, reproduced exactly by `rimraf dist && tsc -p tsconfig.build.json` — verified as an
identical output file set, with both services booting from it. Dev watch is `tsc --watch` alongside
`node --watch`.

Dropping `@nestjs/cli` also left **one** TypeScript in the tree. It pinned `typescript: 5.9.3`
exactly, so every other option — staying on 5.9, or moving to 6 — meant `pnpm typecheck` and
`nest build` running different compilers over the same code.

TS 6.0.3 was the alternative and does keep the CLI working, but it is still the JavaScript compiler:
measured at ~590ms, no faster than 5.9. It buys a version number, not a property.

**Vitest with no SWC step.** The usual Nest recipe adds `unplugin-swc` because esbuild does not emit
decorator metadata, which is exactly what Nest's injector reads. Vite's current transform _does_ emit
it, so the plugin and `@swc/core` are unnecessary — two fewer dependencies and roughly a 9× faster
transform. Worth knowing how that fails, because the symptom does not point at the cause: with
metadata missing, `@nestjs/swagger` reports `A circular dependency has been detected (property key:
"uptimeSeconds")` on a DTO that has no cycle, plus a wall of 500s. If that ever appears, check
`emitDecoratorMetadata` before anything else.

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
  patch (§8, §9). Until then `InMemoryCursorStore` is the only adapter, so **the indexer restarts
  from `INDEXER_START_BLOCK` every time** rather than resuming. The port is real and its resume
  behaviour is tested; the adapter is what is missing. That adapter will also need a
  `withTransaction` seam so a processor's writes and the cursor advance commit together — the gap
  between them is the one at-least-once window the design cannot close on its own.
- **Real reorg detection.** `NoopReorgDetector` never reports a fork, including at bootstrap, so a
  reorg across a restart passes unnoticed. What is needed is a ring buffer of the last
  `FINALITY_DEPTH` headers, a parent-hash walk back to the common ancestor, and a `bootstrap` that
  refills the window from the chain. Note the limitation that will remain: a fork deeper than the
  retention window is _reported_ (`unrecoverable`) rather than repaired, because the blocks it
  invalidates were dispatched as settled ranges and never hash-inspected. That class of corruption is
  what reconciliation exists to catch.
- **Event decoding** — the Spoke and Hub processors, and the Hub asset mirror. That mirror is the
  highest-risk component: one mishandled transition silently corrupts every supply valuation for that
  asset, with no error, just wrong numbers.
- **The reconciliation job** designed in §9, which is what keeps the fold honest over time.
- **Enrichment** and the positions endpoints, per the §12 conclusion. Note that `uint256` amounts
  must be serialised as JSON strings — float64 has 53 bits of mantissa and share balances are far
  past it. The failure mode is a few wei of drift that reads as a rounding bug.
- **Kubernetes manifests.** The probes, drain sequence and JSON logs are already shaped for them.
  CI covers format, lint, typecheck, test and build on Node 24, but does not yet build the Docker
  images — so the `Dockerfile` can rot without anything failing.
