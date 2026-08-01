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
lefthook, the [indexing framework](#indexing) — chain client with provider failover, log reader,
cursor and processor seams, and hash-chain reorg detection over a retained header window, with a
detected fork re-reported on the next start until it has actually been applied — and the shared
ClickHouse layer: client, readiness probe and the [migration runner](#schema-and-migrations).

**Not yet:** any tables to migrate, event decoding, the enrichment source, the positions endpoints,
Kubernetes manifests. The API serves one stub endpoint; the indexer walks the chain and watches for
forks, but its processors are placeholders. See [Not here yet](#not-here-yet).

## Layout

```
.
├── apps/
│   ├── api/                 read API — indexed and enriched positions
│   └── indexer/             worker — configuration and wiring, nothing else
├── docs/
│   └── aave-v4-protocol-analysis.md
├── packages/
│   ├── clickhouse/          the client, its Nest module, a probe, the migration runner
│   ├── indexing/            the chain-agnostic indexing engine
│   │   └── src/
│   │       ├── chain/           RPC access — the ChainClient and LogReader ports
│   │       ├── indexing/        the loop, plus one folder per seam
│   │       │   ├── processors/      what to do with a block range
│   │       │   ├── reorg/           finality, fork detection, and what outlives the process
│   │       │   ├── cursor/          durable position
│   │       │   └── observability/   state machine and health indicator
│   │       └── test-support/    fakes, exported so consumers test against them
│   └── ops/                 probes, logging, graceful shutdown — no domain logic
├── pnpm-workspace.yaml      workspace globs + dependency catalog
├── tsconfig.base.json       one strict compiler configuration, inherited everywhere
├── lefthook.yml             git hooks
└── vitest.config.mts        aggregates every workspace project into one test run
```

Both services are NestJS applications. The API serves HTTP; the indexer is a worker that also
listens, purely so Kubernetes has a probe target — no business endpoints are mounted on it.

**The scope says whether a package knows about Aave.** Everything under `packages/` that does not
is scoped `@packages` — the directory is the whole of its identity, because there is nothing about
this product in it. `@aave-v4-positions` stays on the two apps, which _are_ the product.

`@packages/ops` holds the operational concerns both services share: probes, structured logging and
the shutdown sequence — everything an operator needs and nothing a position needs. The name is the
boundary. It carries no Aave domain knowledge and would work in any Kubernetes-deployed Nest service,
so the share maths becomes its own package rather than accumulating here. Wrapping `nestjs-pino`
there also means no app depends on it directly.

`@packages/clickhouse` is the database layer on the same terms — the client, its Nest module, a
readiness probe and the [migration runner](#schema-and-migrations), and nothing that knows what is
stored in it. Repositories live with whatever owns their tables and inject the client from here, so
this package never becomes a catalogue of every table in the system.

`packages/indexing` is the [loop](#indexing) and the seams it drives. It knows about block numbers,
forks and cursors, and nothing about Aave — a processor is something a consumer writes. It had to
leave `apps/indexer` for that to be true at all: a package cannot import from an app. What remains
in the app is about 220 lines — `main.ts`, `AppModule` and env validation — and everything it _does_
comes from the packages it wires together.

`pnpm -r` walks the workspace in topological order, so each package builds before its consumers with
no extra wiring. Consumers deliberately read **source** instead of `dist`: every vitest config
aliases the workspace packages, one entry each, so tests can never exercise a stale build.
`pnpm typecheck` is the exception — it builds the packages first and checks against the emitted
`.d.ts`, which is what actually verifies the surface consumers see.

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

| variable                     | default    |                                                                                                              |
| ---------------------------- | ---------- | ------------------------------------------------------------------------------------------------------------ |
| `CHAIN_ID`                   | _required_ | Checked against what the providers report, on the first iteration.                                           |
| `RPC_URLS`                   | _required_ | Comma-separated, tried in order.                                                                             |
| `FINALITY_DEPTH`             | `128`      | The reorg detector's, never the loop's: it sets both the settled boundary and how many headers are retained. |
| `INDEXER_START_BLOCK`        | `24720899` | Main Spoke genesis; used only when no cursor exists.                                                         |
| `INDEXER_MAX_RANGE_SIZE`     | `1000`     | Blocks per dispatch while catching up.                                                                       |
| `INDEXER_POLL_INTERVAL_MS`   | `4000`     |                                                                                                              |
| `INDEXER_RPC_TIMEOUT_MS`     | `10000`    |                                                                                                              |
| `INDEXER_STALL_THRESHOLD_MS` | `300000`   | How long without progress before readiness fails.                                                            |
| `INDEXER_AUTOSTART`          | `true`     | `false` boots the probes without indexing.                                                                   |

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

## Schema and migrations

**Migrations are `.sql` files**, including the one that creates the ledger they are recorded in — so
the schema reads as schema: reviewable as a diff, runnable by hand when something needs checking,
and not something a refactor of the surrounding TypeScript can quietly alter. **One statement per
file**, because ClickHouse's HTTP interface takes one per request and splitting on `;` means writing
a SQL parser that has to know about semicolons inside string literals and comments — a parser nobody
would think to test until it silently truncated a migration. A table and the view over it are
therefore two files, which also means each is recorded and retried independently.

Each package owns the migrations for the tables it defines, and the application names the
directories that deploy together. The runner orders by ordinal _across_ directories rather than
grouping by package, and rejects a set whose `NNN_` ordinals collide, naming both sides. Without
that last part two packages could each reach for `002` without either author noticing, and the apply
order would quietly depend on how the application happened to list the directories.

Applying the schema is **its own step, never something a service does at boot** — two replicas
starting together would race each other through the same DDL.

## Indexing

The indexer walks the chain and hands block ranges to registered processors. The loop itself knows
about block numbers and nothing else — no notion of finality, of what a fork looks like, or of what
a processor does with a range. Each of those sits behind a port.

**Three seams, all injected at module setup.**

```ts
interface BlockProcessor {
  // both inclusive [from, to]: index this range / discard this range
  onBlockRange(from, to, signal): ProcessorOutcome | Promise<ProcessorOutcome>;
  onReorg(from, to, signal): ProcessorOutcome | Promise<ProcessorOutcome>;
}
interface ReorgDetector {
  bootstrap(cursor): Promise<ReorgVerdict>; // vet the resume point against the chain
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
calls `safeHead()` and uses the answer for one thing — **how wide to dispatch**. At or below the
boundary the blocks are settled, so they go out in ranges, which is what makes a ~932k-block backfill
take minutes; above it, one at a time. That boundary is recomputed from a fresh head every iteration
rather than latched as a mode, and a range never straddles it. Swapping the depth heuristic for
`getBlock({ blockTag: 'finalized' })` later changes one file.

Width is all the loop decides. **Every header it commits goes to `inspect()` first**, settled or not,
and whether that header needs checking is the detector's call — it knows where the boundary is and
waves a settled top through without looking. Which is not the same as inspecting less: a range yields
one header for a thousand blocks, so a backfill asks ~940 questions rather than 932k. Leaving the
loop to decide what was worth asking about would have put a finality judgement in the one component
that is supposed to hold none.

**Processors gate the cursor.** They return `ok` / `retry` / `failed` rather than throwing, and run
sequentially in registration order, stopping at the first non-`ok`. The cursor advances only when all
of them succeeded, and it is saved **last** — it is the single durable commit point, so a crash
anywhere earlier replays the range rather than skipping it. Unwinding a fork is the one place
something follows it, and for the same reason: the detector's rewind discards the headers that are
the evidence of the abandoned branch, so it waits until the cursor is durable. The corollary is that
dispatch is **at-least-once per processor per range**, and processors must be idempotent. That is not a wart: the
analysis already models positions as a fold over an immutable log, so replay is the repair primitive
(§8, §9.4).

A `retry` may set `narrowRange`, which halves the range size — the measured provider caps above are
why. Retries are unbounded, because an RPC outage should recover on its own; the bound is
observability, not control, and `/health/ready` fails once the loop has made no progress for
`INDEXER_STALL_THRESHOLD_MS`. `failed` is reserved for "more attempts cannot help" and is terminal.

**Detection is free in the steady state.** `inspect` compares the new block's `parentHash` against
the hash retained for the block below it — both already in hand, so a chain that simply extends
costs no RPC call at all. Only a mismatch opens a backwards walk, and only for as many calls as the
fork turns out to be deep. The window is `FINALITY_DEPTH + 1` headers deep, and that extra entry is
not slack: the deepest fork the loop can hand over begins at `safeHead + 1`, so placing it means
matching its parent at exactly `safeHead`.

**The window is one contiguous run, enforced rather than assumed.** A walk over a set with holes in
it is not a parent-hash chain; it is spot checks joined by an assumption. The assumption does hold —
a chain is ancestor-closed, so a header that still matches proves its ancestors do — but leaning on
it makes a hole indistinguishable from a healthy window, and the walk then rewinds past blocks it
can say nothing about. So the run is kept whole at the point it is written rather than guarded at
the point it is read: `commit` restarts the window whenever a header does not continue it, and a
restarted window immediately **pulls its predecessors back in**. The walk itself carries no
gap-handling, because there is no gap to handle.

"Continues it" is a hash question, not a numeric one. Adjacent block numbers over mismatched hashes
are two branches stacked on each other, which is worse than a hole because it still looks walkable —
the walk would find a block it thinks is shared, and under-report the fork. `inspect` makes that
comparison for every block at the tip, but a settled range is dispatched without inspection, so
`commit` makes it too, for nothing, from data already in hand.

The pull is what keeps a restart from costing depth. While catching up the loop commits only the top
header of each range, and each of those lands clear of the last, so the window would otherwise
collapse to a single anchor and have to earn its depth back one block at a time. Instead the
predecessors are followed down by `parentHash` — each link checked, so every one of them is proven
rather than read on trust. It is skipped below the boundary, where a predecessor is settled and
nothing could ever consult it: that is not thrift but necessity, since pulling `FINALITY_DEPTH`
headers per dispatched range would cost more calls than the backfill itself. In practice it fires
once, on the range that lands on the boundary, and the window is full from the first inspected block.

A hole is therefore outside the model — nothing the loop does can produce one, and a test drives the
window through every shape the loop can leave it in to say so. Only a store that lost rows could,
and defending the walk against that would mean carrying a branch no test can reach.

**A reported fork is owed until it is applied, and the window is what remembers.** A verdict handed
to the loop lives only as long as the process, so a crash mid-unwind must not lose it. Nothing extra
is written down for that, because the cursor and the window already say it between them: while an
unwind is unfinished the two disagree, and the disagreement is the record.

**The rewind is the last step, after the cursor is durable.** A rewind discards the retained headers,
and those headers are the evidence of the branch being abandoned — throwing them away for a write
that has not landed yet gets the order backwards. So the gap the loop can die in leaves the cursor
already correct and the window still holding the run above it, which is strictly more to work with
than the reverse would leave.

Every point the process can die therefore lands on the same answer. Before the discard was
dispatched, after it, or after the cursor save — `bootstrap` vets the window's top, walks to the same
ancestor and reports the same range, the loop re-dispatches, and `onReorg` is an idempotent discard.
Only once the rewind has also run does the question stop being asked.

The same check covers a fork that happened while the process was down and was never detected at all —
same cursor, same window, same answer. There is one condition here, not two. A separate record of
"a reorg is owed" would have to be durable to help, and would then be repeating what the cursor and
the window already say, with the added job of never drifting out of step with them.

**Resume is a detector question too.** On start the loop loads the cursor and hands it to
`bootstrap()`, which reads the chain's header at that height and compares it against
`cursor.lastHash` — the one record of the branch we actually followed that survives a restart
unaided. A match settles it: a chain is ancestor-closed, so a canonical block makes everything
beneath it canonical too, and the ordinary resume costs exactly one call. A mismatch means the
process was stopped across a fork, and the retained headers are what locate it.

The block it vets is the **window's top, which is not always the cursor** — the cursor is only the
fallback for an empty window. Two things leave the window ahead: `commit` runs before the cursor is
saved, so a rejected save leaves it one block up with those events already in the projection, and an
unwind that saved the cursor but died before rewinding leaves it a whole run up. If the chain then
replaces exactly that block its parent is untouched, so the cursor still looks perfectly canonical —
checking it alone would answer continuous and fold the replacement on top of a branch nothing ever
discarded. `inspect` guards the same height for the same reason, since the loop replays that block
whether or not the process restarted in between.

On the matching path the resume point becomes the window's anchor and the pull above rebuilds
beneath it, so the indexer starts full-depth rather than blind. On the mismatch path it cannot, and
that asymmetry is the point rather than a shortfall. Reading headers by height and trusting them
looks identical to following verified links and is worth nothing: once the resume point has been
reorged out, those headers describe the branch that won, and recording them would erase the only
evidence of the branch that lost — leaving every fork looking one block deep. That path has whatever
was durably retained and nothing else, which is why the window sits behind its own port,
`BlockHeaderStore`; `InMemoryBlockHeaderStore` is the only adapter today.

**Provider failover is viem's `fallback`**, tried in list order. Two of its defaults are overridden,
both verified against the 2.55 source rather than the docs: `fallback`'s own `retryCount` goes to 0
so a total outage surfaces immediately instead of after four full sweeps of the list, and the head is
clamped monotonically in the loop — viem does not reconcile block height between providers, so
failing over to a lagging node otherwise looks exactly like a reorg.

**Reading logs is a fourth port, and not a seam of the loop.** `IndexerService` never reads a log;
processors do. `LogReader` is therefore a separate interface rather than a third method on
`ChainClient`, which would have made every loop spec grow a method the loop never calls. The module
binds it from the RPC configuration it already holds, so a processor injects `LOG_READER` without
the application configuring anything further.

**One adapter per port.** `ViemLogReader` extends `ViemChainClient` for the transport and the header
read, but is bound as its own provider, so what a consumer can do is decided by the token it
injects — the loop cannot read a log, a processor cannot move the loop. Nothing outside `chain/`
ever holds a viem client. The two connections are built from the same ordered provider list and try
it in the same order, so they diverge only if one fails over and the other does not; the monotonic
head clamp is what keeps that from reading as a reorg.

The port hides two viem behaviours, both measured. viem's typed `getLogs` **silently ignores a
`topics` argument** — the same 124 logs came back filtered and unfiltered — so a filter that appears
to narrow to a handful of events would in fact fetch everything a contract emits; the adapter uses
`client.request`. And `eth_getLogs` returns `blockTimestamp` on some providers but not all, and viem
types it optional: the port makes it unconditional and the adapter fills it from headers when a
provider omits it, one read per distinct block rather than per log.

`LogRangeTooLargeError` is matched on the provider's message, not its error class or JSON-RPC code,
because neither separates the cases — 1rpc's range rejection and publicnode's archive-plan refusal
are both `InvalidParamsRpcError` with code `-32602`. The match needs a range word _and_ a limit word
together, so a rate-limit message carrying only the second does not narrow the range. Both halves
are pinned by tests built from the literal strings four public endpoints returned.

**What is real and what is not.** The chain client, the log reader, the loop, the cursor seam, the
outcome protocol and reorg detection all work. `LoggingBlockProcessor` only logs, so nothing is
decoded or stored yet, and the loop's own failure paths are exercised by tests through scripted
fakes rather than by the running service.

Three limits are worth stating plainly:

- **A fork reaching at or below the safe head is reported, not repaired.** Those blocks went out as
  settled ranges and were never hash-inspected, so the detector answers `unrecoverable` rather than
  guessing which of them are wrong. That class of corruption is what reconciliation exists to catch.
- **A head that jumps well ahead re-enters wide ranges** with no ancestry check on the block the
  cursor sits at — the boundary moves above it and it becomes settled by arithmetic alone.
- **No fork survives a restart with an in-memory window**, whether it was detected first or not.
  Both cases need the same thing — retained headers to walk — and the rebuild cannot supply them,
  by construction: the resume point is exactly what was reorged out, so the chain can no longer say
  what we processed. `bootstrap` holds one hash, finds nothing retained, and answers
  `unrecoverable`. It costs nothing today, since the cursor is in memory too and there is no resume
  to get wrong, but it is why the cursor and the window want to become durable in the same change.

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

- **Tables.** The ClickHouse layer is here; nothing defines a table through it yet. The analysis
  settles what has to be stored — immutable raw events plus a derived projection, which is also what
  makes reorg handling a replay rather than a patch (§8, §9). Until then `InMemoryCursorStore` is
  the only adapter, so **the indexer restarts from `INDEXER_START_BLOCK` every time** rather than
  resuming. The port is real and its resume behaviour is tested; the adapter is what is missing.
  That adapter will also need a `withTransaction` seam so a processor's writes and the cursor
  advance commit together — the gap between them is the one at-least-once window the design cannot
  close on its own. It should bring a `BlockHeaderStore` adapter with it rather than leaving it to
  follow: those two are what make an unapplied reorg re-derivable, and a durable cursor over an
  in-memory window turns every cross-restart fork into `unrecoverable`, which is strictly worse than
  today, where nothing survives and there is no resume to get wrong.
- **Event decoding** — the Spoke and Hub processors, and the Hub asset mirror. That mirror is the
  highest-risk component: one mishandled transition silently corrupts every supply valuation for that
  asset, with no error, just wrong numbers.
- **The reconciliation job** designed in §9, which is what keeps the fold honest over time.
- **Enrichment** and the positions endpoints, per the §12 conclusion. Note that `uint256` amounts
  must be serialised as JSON strings — float64 has 53 bits of mantissa and share balances are far
  past it. The failure mode is a few wei of drift that reads as a rounding bug.
- **Metrics export.** `IndexerStatus` knows the cursor, the head, the lag between them and the
  consecutive-failure count, but the only way out is `/health/ready`, which answers up or down and
  puts the detail in an error string. That is a fine alert and a poor time series — you cannot graph
  indexing lag or alert on it before it crosses the stall threshold. The shape when it lands is a
  **write-only** observer, optional and multi-provider like the processors, called from the loop's
  state transition:

  ```ts
  interface IndexerObserver {
    onProgress(snapshot: IndexerSnapshot): void;
    onRetry(reason: string, snapshot: IndexerSnapshot): void;
    onFailure(reason: string, snapshot: IndexerSnapshot): void;
  }
  ```

  Write-only is the whole point. `IndexerStatus` itself is deliberately **not** a port, unlike the
  processor, reorg and cursor seams: the loop reads back from it — `isFailed` to know it is
  finished, `observeHead` for the clamped value it indexes against, `consecutiveFailures` to size
  the backoff — so swapping the implementation would change correctness rather than policy. An
  observer that nothing reads back from costs a metric when it misbehaves, not a stalled indexer.

- **Kubernetes manifests.** The probes, drain sequence and JSON logs are already shaped for them.
  CI covers format, lint, typecheck, test and build on Node 24, but does not yet build the Docker
  images — so the `Dockerfile` can rot without anything failing.
