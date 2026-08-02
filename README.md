# aave-v4-positions

[![CI](https://github.com/EugeneButusov/aave-v4-positions/actions/workflows/ci.yml/badge.svg)](https://github.com/EugeneButusov/aave-v4-positions/actions/workflows/ci.yml)

An indexer and read API for **Aave v4 user positions** on the Hub-and-Spoke architecture.

The protocol groundwork is in [`docs/aave-v4-protocol-analysis.md`](docs/aave-v4-protocol-analysis.md):
where position state actually lives in v4, which events reconstruct it, the share/asset maths, the
health-factor and price layers, and the ingestion constraints. Everything in this repository is built
against those findings rather than v3 intuition.

## Status

The **indexing framework, real event ingestion, and the fold that turns it into positions**. The
workspace, both services, the test and lint toolchain, the operational shape a Kubernetes deployment
expects — and a loop that walks Ethereum mainnet decoding Aave v4 Spoke position events into an
append-only ClickHouse table, projected into per-wallet balances that survive a reorg without any
code being told one happened.

**Present:** pnpm workspace, two runnable NestJS services, validated configuration, structured
logging, liveness/readiness probes, graceful drain, OpenAPI docs, Vitest, oxlint + Prettier,
lefthook, the [indexing framework](#indexing) — chain client with provider failover, log reader,
cursor and processor seams, and hash-chain reorg detection over a retained header window, with a
detected fork re-reported on the next start until it has actually been applied; a durable cursor and
header window in Postgres, so a restart resumes instead of re-indexing; the shared ClickHouse
layer of client, readiness probe and [migration runner](#schema-and-migrations); and
[event ingestion](#event-ingestion) — the eight Main Spoke events that move a position and the
thirteen [Core Hub events](#the-hub-ledger-and-why-it-is-a-second-table) that will value them,
decoded against the official ABIs into two append-only ledgers;
[the position fold](#the-position-fold) over the Spoke ledger, with a keyset-paginated store to read
it; and [the Hub asset fold](#the-hub-asset-fold) over the Hub's, reconciled against the chain's
own `getAsset`.

**Not yet:** the share→asset arithmetic, and therefore any valuation on the read path — position
_type_ as §12.1 defines it, prices, health factors, the positions endpoints, Kubernetes manifests.
Balances are still shares: both halves of the conversion are now indexed, and nothing multiplies
them together yet. The API still serves one stub endpoint. See [Not here yet](#not-here-yet).

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
│   ├── postgres/            the same four things for the indexer's own state
│   ├── migrations/          reads and orders .sql files; no client, so both runners share it
│   ├── indexing/            the chain-agnostic indexing engine
│   │   └── src/
│   │       ├── chain/           RPC access — the ChainClient and LogReader ports
│   │       ├── indexing/        the loop, plus one folder per seam
│   │       │   ├── processors/      what to do with a block range
│   │       │   ├── reorg/           finality, fork detection, and what outlives the process
│   │       │   ├── cursor/          durable position
│   │       │   └── observability/   state machine and health indicator
│   │       ├── postgres-migrations/  the two tables the Postgres adapters own
│   │       └── test-support/    fakes and port contracts, exported so adapters run them
│   ├── ops/                 probes, logging, graceful shutdown — no domain logic
│   └── aave-positions/      packages that know about Aave
│       ├── events/          ABI bindings, decoders, two append-only event ledgers
│       └── positions/       the fold over that log, and the store that reads it
├── pnpm-workspace.yaml      workspace globs + dependency catalog
├── tsconfig.base.json       one strict compiler configuration, inherited everywhere
├── lefthook.yml             git hooks
└── vitest.config.mts        aggregates every workspace project into one test run
```

Both services are NestJS applications. The API serves HTTP; the indexer is a worker that also
listens, purely so Kubernetes has a probe target — no business endpoints are mounted on it.

**The scope says whether a package knows about Aave.** Everything under `packages/` that does not
is scoped `@packages` — the directory is the whole of its identity. Anything that _does_ know is
nested under `packages/aave-positions/` and scoped `@aave-positions`, so the import line says
whether a module is protocol-aware before you open it. `@aave-v4-positions` stays on the two apps,
which _are_ the product.

`@packages/ops` holds the operational concerns both services share: probes, structured logging and
the shutdown sequence — everything an operator needs and nothing a position needs. The name is the
boundary. It carries no Aave domain knowledge and would work in any Kubernetes-deployed Nest service,
so the share maths becomes its own package rather than accumulating here. Wrapping `nestjs-pino`
there also means no app depends on it directly.

`@packages/clickhouse` is the database layer on the same terms — the client, its Nest module, a
readiness probe and the [migration runner](#schema-and-migrations), and nothing that knows what is
stored in it. Repositories live with whatever owns their tables and inject the client from here, so
this package never becomes a catalogue of every table in the system.

`@packages/postgres` is the same package for the other database, and `@packages/migrations` is what
fell out when there were two: reading and ordering `.sql` files needs no client, and two copies of the
ordinal-collision rule would drift silently.

`@packages/indexing` is the [loop](#indexing) and the seams it drives. It knows about block numbers,
forks and cursors, and nothing about Aave — a processor is something a consumer writes. It had to
leave `apps/indexer` for that to be true at all: a package cannot import from an app.

Each seam folder holds its port and the adapter behind it side by side — `cursor/` has `CursorStore`
and `PostgresCursorStore`, `reorg/` the same for the header window. The two tables those adapters own
ship as `.sql` from `src/postgres-migrations/`, and the application names the directory. There is
**one** adapter per persistence port, not two: an in-memory cursor under a durable window (or the
reverse) names a resume point nothing can vet, so the second one would exist only to be wired by
mistake. The in-memory doubles live in `test-support/` with the other fakes, and they run the same
contract suites the real adapters do — a fake that quietly disagrees with its port turns every test
using it into a proof about a fiction.

`@aave-positions/events` is the first package on the other side of that line. It ships
`SpokeEventsModule`: the ClickHouse client, the event store and the block processor that fills it,
behind one `forRootAsync`. The application says which Spoke to follow and hands the exported
processor to the loop; it assembles none of the parts.

`@aave-positions/positions` is [the fold](#the-position-fold) over that log: materialized views that
turn events into balances, and a read store over them. It is deliberately thin next to the events
package, and the asymmetry is the design — ingestion is code, so that package owns a processor and a
write path, while this one owns a query, because the database maintains the projection. There is
nothing here to start and nothing to keep in step with the indexer.

`apps/indexer` is left with about 310 lines: `main.ts`, `AppModule`, env validation and the
migration entry point. Everything it _does_ comes from the packages it wires together, which is what
makes the engine reusable and the Aave half independently testable. It is also the composition point
for schema: each package owns the migrations for its own tables, and the application declares which
sets ship together — the same reason it is the application that names which processors to run.

`pnpm -r` walks the workspace in topological order, so each package builds before its consumers with
no extra wiring. Consumers deliberately read **source** instead of `dist`: every vitest config
aliases the workspace packages, one entry each, so tests can never exercise a stale build.
`pnpm typecheck` is the exception — it builds the packages first and checks against the emitted
`.d.ts`, which is what actually verifies the surface consumers see.

## Prerequisites

- **Node 24** — enforced by `engines` with `engine-strict` on, so an older runtime fails at install
  rather than at runtime. It is the only version CI runs, so the declaration and the evidence match.
- **pnpm 11** — `corepack enable` picks up the `packageManager` field automatically.
- **Docker**, to run the tests. The store specs go against real servers rather than mocks: what they
  assert is that the SQL executes — the collapsing semantics on one side, upsert-and-prune in a single
  data-modifying CTE on the other — and neither is something a fake can tell you. CI runs the same
  images as service containers.

  ```bash
  docker run -d --rm --name clickhouse -p 8123:8123 -e CLICKHOUSE_SKIP_USER_SETUP=1 clickhouse/clickhouse-server:26.3-alpine
  ```

  ```bash
  docker run -d --rm --name postgres -p 5432:5432 -e POSTGRES_HOST_AUTH_METHOD=trust postgres:18-alpine
  ```

  `CLICKHOUSE_SKIP_USER_SETUP` and `POSTGRES_HOST_AUTH_METHOD` both drop the password, so the
  defaults in the vitest configs need no configuration at all.

There is no Nest CLI. `pnpm build` is `tsc`, and `pnpm dev:*` is `tsc --watch` plus `node --watch`.

## Getting started

```bash
pnpm install
```

That also installs the git hooks (via the `prepare` script). Then give each service an environment:

```bash
cp apps/api/.env.example apps/api/.env && cp apps/indexer/.env.example apps/indexer/.env
```

The indexer needs a ClickHouse to write to (see [Prerequisites](#prerequisites)) with the schema
applied. It never migrates at boot, so this is a step:

```bash
pnpm --filter @aave-v4-positions/indexer migrate
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

Five services: ClickHouse and Postgres, a one-shot `migrate` that applies both schemas and exits,
then the API and the indexer. `docker compose ps` shows the long-running four as `healthy` once their
probes pass. Same addresses as above (`:3000`, `:3001`, `/docs`), ClickHouse on `:8123`, Postgres on
`:5432`. Tear down with `docker compose down`, or `down -v` to drop the indexed data and the cursor
with it — after which the next start re-indexes from `INDEXER_START_BLOCK`.

The indexer waits for `migrate` to succeed rather than migrating at boot — replicas would otherwise
race the same DDL — and then starts indexing against **`https://eth.drpc.org` by default**, so
`docker compose up` makes real requests to a third party. Point it elsewhere with
`RPC_URLS=https://your-node docker compose up`, or set `INDEXER_AUTOSTART=false` for probes only.

`docker compose logs -f indexer` is the quickest way to watch it work: 1000-block ranges up from the
Main Spoke genesis, the last one self-truncating to stop exactly on the finality boundary, then one
block at a time at the tip. A full backfill is ~94 chunked requests and lands roughly 57k rows; to
skip it while trying the stack out, start a few thousand blocks below the tip:

```bash
INDEXER_START_BLOCK=25656868 docker compose up --build
```

Then look at what it stored:

```bash
docker compose exec clickhouse clickhouse-client --user aave --password aave --database aave --query "SELECT event_name, count() FROM spoke_events_current GROUP BY event_name ORDER BY 2 DESC"
```

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
| `pnpm backfill --from X --to Y`     | push one block range through the processors  |
| `pnpm test`                         | every project's tests in one run             |
| `pnpm test:watch` / `pnpm test:cov` | watch mode / V8 coverage                     |
| `pnpm typecheck`                    | `tsc --noEmit` across the workspace          |
| `pnpm lint` / `pnpm lint:fix`       | oxlint, type-aware                           |
| `pnpm format` / `pnpm format:check` | Prettier                                     |
| `pnpm check`                        | format, lint, typecheck, test — what CI runs |
| `pnpm clean`                        | remove build output                          |

Applying the ClickHouse schema is its own command, never something a service does at boot:

```bash
pnpm --filter @aave-v4-positions/indexer migrate
```

Checking the Hub fold against the chain is its own command too, and it runs against whatever RPC
you point it at rather than on a schedule — see [the fold's verification](#verification-1):

```bash
pnpm --filter @aave-v4-positions/indexer reconcile:hub -- --from X --to Y
```

Scope anything to one service with `pnpm --filter @aave-v4-positions/api <script>`.

## Configuration

Every variable is parsed by a Zod schema at boot. An invalid value **aborts the process** rather than
defaulting silently — a pod that crash-loops on bad config is far easier to diagnose than one quietly
indexing the wrong chain. Deployed environments inject variables directly; the `.env` file is a
local-development convenience and is skipped entirely under `NODE_ENV=test`.

**Shared** — `NODE_ENV`, `LOG_LEVEL`, `LOG_PRETTY`, `SHUTDOWN_GRACE_SECONDS`.

**API** — `API_HOST`, `API_PORT` (3000), `API_GLOBAL_PREFIX` (`api`), `API_DOCS_PATH` (`docs`).

**Indexer** — `INDEXER_HOST`, `INDEXER_PORT` (3001), plus the chain configuration:

| variable                     | default    |                                                                                                                      |
| ---------------------------- | ---------- | -------------------------------------------------------------------------------------------------------------------- |
| `CHAIN_ID`                   | _required_ | Checked against what the providers report, on the first iteration.                                                   |
| `RPC_URLS`                   | _required_ | Comma-separated, tried in order.                                                                                     |
| `FINALITY_DEPTH`             | `128`      | The reorg detector's, never the loop's: it sets both the settled boundary and how many headers are retained.         |
| `INDEXER_START_BLOCK`        | `24720891` | Core Hub genesis, the earlier of the two contracts. Read on a cold start and never again; see [Resuming](#resuming). |
| `INDEXER_MAX_RANGE_SIZE`     | `1000`     | Blocks per dispatch while catching up.                                                                               |
| `INDEXER_POLL_INTERVAL_MS`   | `4000`     |                                                                                                                      |
| `INDEXER_RPC_TIMEOUT_MS`     | `10000`    |                                                                                                                      |
| `INDEXER_STALL_THRESHOLD_MS` | `300000`   | How long without progress before readiness fails.                                                                    |
| `INDEXER_AUTOSTART`          | `true`     | `false` boots the probes without indexing.                                                                           |
| `MAIN_SPOKE_ADDRESS`         | Main Spoke | Which Spoke to follow. A second Spoke is a second registration, not an edit.                                         |
| `CORE_HUB_ADDRESS`           | Core Hub   | Which Hub to follow, for the asset state that turns shares into balances.                                            |

**Storage** — `CLICKHOUSE_URL`, `CLICKHOUSE_DATABASE`, `CLICKHOUSE_USER`, `CLICKHOUSE_PASSWORD` for
the event log, and `POSTGRES_URL` (`postgres://postgres@localhost:5432/postgres`) for the indexer's
own position. One URL rather than four fields, because every managed Postgres hands you exactly that
string, often carrying `?sslmode=require`, and taking it apart only to reassemble it is how a
percent-encoded password gets mangled.

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
and not something a refactor of the surrounding TypeScript can quietly alter.

**A file is one migration, recorded once, however many statements it holds.** ClickHouse's HTTP
interface refuses multi-statement requests outright — `Multi-statements are not allowed`, measured —
so the runner splits, and it splits on a `--@statement` marker the author writes rather than on `;`.
These files carry semicolons inside comments and inside string literals, so splitting on the
character means writing a SQL lexer that knows about quoting, escapes and both comment forms — one
nobody would think to test until it silently truncated a migration. A marker needs no lexer, cannot
collide with SQL content, and is itself a comment, so the file stays valid SQL.

Statements still go in separate files unless they are meaningless apart from each other. A table and
the view over it are two migrations; [the fold](#the-position-fold)'s nine projections of one table
are one, because they are one change to make and one change to review. Recording happens once the
whole file lands, so a file that fails part-way is retried from its first statement — which is why
every statement in a multi-statement file is written `IF NOT EXISTS`.

Each package owns the migrations for the tables it defines — the `spoke_events` and `hub_events`
ledgers and their views belong to the events package, the projections over it to the positions package, the cursor and header window to
`@packages/indexing` — and the application names the directories that deploy together. That
cross-directory ordering is load-bearing rather than tidy: the projections read a table another
package creates, and `010 > 002` is the entire guarantee that they are created after it. The runner
orders by ordinal _across_ directories rather than grouping by package, and rejects a set whose
`NNN_` ordinals collide, naming both sides. Without that last part two packages could each reach for
`002` without either author noticing, and the apply order would quietly depend on how the application
happened to list the directories.

**Two databases mean two ordinal namespaces**, and `migrate` loads them as two separate sets for
exactly that reason: the event log's `001_spoke_events` and the cursor's `001_indexer_cursor` are not
a collision, and merging them to "apply everything at once" would both reintroduce one and try to run
Postgres DDL against ClickHouse. Each half logs under its own name so a failure says which database
it was.

**The engine is in the directory name** — `clickhouse-migrations/`, `postgres-migrations/` — because
nothing else about a migration says which one it is for. Both runners take the same
`NNN_snake_case.sql` shape, and a set is only valid against the database it was written for. It is
the directory rather than the filename because the ordinal has to lead the filename, so a prefix
there would sit in the middle and group nothing; and because the recorded id _is_ the filename, so
renaming files would make every migration look pending again. The two database packages keep a plain
`migrations/`: `@packages/clickhouse` and `@packages/postgres` already say it in their own names.

The two runners differ in one way worth knowing. Postgres DDL is transactional, so a failed run there
leaves _nothing_ — no half-created table to drop by hand before retrying — and it takes a
`pg_advisory_xact_lock` so two runners started together serialise. ClickHouse can offer neither, so it
records each migration only after the statement lands and retries from there.

Applying the schema is **its own step, never something a service does at boot** — two replicas
starting together would race each other through the same DDL. Compose runs `migrate` as a one-shot
service the indexer waits on.

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
`BlockHeaderStore`, and why the adapter that ships behind it is the Postgres one.

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
outcome protocol, reorg detection, the event processor and the durable state stores all work. The
loop's own failure paths are exercised by tests through scripted fakes rather than by the running
service.

Three limits are worth stating plainly:

- **A fork reaching at or below the safe head is reported, not repaired.** Those blocks went out as
  settled ranges and were never hash-inspected, so the detector answers `unrecoverable` rather than
  guessing which of them are wrong. That class of corruption is what reconciliation exists to catch.
- **A head that jumps well ahead re-enters wide ranges** with no ancestry check on the block the
  cursor sits at — the boundary moves above it and it becomes settled by arithmetic alone.
- **Nothing enforces a single writer.** Two indexers on one `chain_id` now share a cursor row and a
  window rather than each keeping their own, which a rolling deploy produces for as long as the old
  pod takes to drain. The damage is bounded — processors are idempotent and both write the same
  canonical headers — but the cursor can move backwards and cost a re-index. See
  [Not here yet](#not-here-yet).

### Resuming

The cursor and the retained window are two small tables in Postgres — one row, and `FINALITY_DEPTH +
1` more, per chain. They landed together on purpose: a durable cursor over an in-memory window would
be **strictly worse than neither**, because the resume point is exactly what a fork reorged out, so
`bootstrap` would hold one hash, find nothing retained, and answer `unrecoverable` for every
cross-restart fork.

Nothing wraps the two writes, and nothing needs to. The loop commits the header to the window and
_then_ saves the cursor, and rewinds only _after_ saving it — so whichever of the two a crash lands
between, the window is at or ahead of the cursor, which is the state `bootstrap` is built to resolve.
A window _behind_ its cursor is the one shape the detector calls corruption rather than a fork. What
that ordering does require is that neither adapter buffers: a write-behind cursor would break it.

There is deliberately **no `withTransaction` seam**, which earlier notes here promised. It would have
to commit a processor's writes together with the cursor advance, and processors write to ClickHouse
while the cursor lives in Postgres — so no cursor store anywhere could do it. The at-least-once
window stays open and idempotence is the whole answer rather than a stopgap.

Two consequences for operators:

- **`INDEXER_START_BLOCK` is consulted on a cold start and never again.** Raising it later to skip
  history does nothing while a cursor row exists.
- **Resetting is two deletes**, with the indexer stopped first — a running loop rewrites the row
  within a poll interval:

  ```sql
  DELETE FROM indexer_cursor        WHERE chain_id = 1;
  DELETE FROM indexer_block_headers WHERE chain_id = 1;
  ```

  Deleting only the cursor is safe: `bootstrap` clears the window itself. Deleting only the window
  usually is too — it refills from the chain while the cursor still matches — but if that block has
  since been reorged out, the detector answers `unrecoverable` and wants the pair.

### Backfilling a range on demand

"Backfill" means two things here. The loop backfills to _reach_ the tip: once, from wherever its
cursor sits, as fast as wide ranges allow. The command backfills a range you _name_, and moves
nothing:

```bash
pnpm backfill --from 24720899 --to 24730899
pnpm backfill --from 24720899 --to 24730899 --dry-run
```

It exists for work the loop cannot be asked for — a processor whose decoding was wrong over a known
range, a processor that joined after the loop had walked past the history it needs, a range being
checked by hand. Re-running a range is defined behaviour rather than a repair: dispatch is
at-least-once and processors are required to be idempotent, so this needs no coordination with a
running indexer.

It shares the indexer's wiring — same config, same processors, resolved through the same DI — and
deliberately not its state:

- **No cursor.** `BackfillRunner` does not inject `CursorStore`, so it cannot move a running
  indexer's resume point. Interrupted, it prints the block to pass to `--from` next.
- **No reorgs.** It injects the detector narrowed to `Pick<ReorgDetector, 'safeHead'>` — the one
  question with no side effect — so the compiler, not a comment, is what stops it committing or
  rewinding a header window.
- **Nothing above the safe head.** Having no way to unwind a fork, it refuses a range reaching into
  the unsettled zone rather than clamping it, and names the highest block it would accept. The tip
  is the loop's job.

Retries are bounded here (`--max-attempts`, default 5) where the loop's are unbounded. Unbounded is
right for a daemon, whose wedging surfaces on `/health/ready`; a command that never gives up just
hangs a terminal with nothing watching it.

`--processors` narrows a run to a subset, matched against `BlockProcessor.name`. The Spoke processor
names itself after the address it follows, so that is what to pass — quoted, since it contains
parentheses:

```bash
pnpm backfill --from 24720899 --to 24730899 --processors 'aave-events(0x94e7a5dc)'
```

`--help` lists the rest.

## Event ingestion

`@aave-positions/events` turns Main Spoke logs into rows. It reads the range the loop dispatched with
one `eth_getLogs`, decodes against the official ABI, and writes.

**`SpokeEventsModule` is the whole surface an application uses.** It owns the ClickHouse client, the
store and the processor, and exports the processor for `IndexingModule`'s `processors`. Following a
second Spoke is a second call with a different address and its own token — nothing else changes,
which is the property the split exists for. It binds its own `LOG_READER` rather than borrowing the
loop's: a module in `IndexingModule`'s `imports` cannot reach what that module provides, since
dynamic-module exports flow outward to importers and not inward.

**Eight events**, exactly what §12.2 lists as the position fold's inputs plus the registry that gives
`reserveId` a meaning: `Supply`, `Withdraw`, `Borrow`, `Repay`, `LiquidationCall`, `ReportDeficit`,
`SetUsingAsCollateral`, `AddReserve`. Three protocol traps each carry a test rather than a comment —
positions key on `user` and never `caller`, decoding is scoped by emitting address because
`ReportDeficit` exists on both Spoke and Hub with different signatures, and the position managers'
fold events are excluded because folding them would double-count every routed action.

**ABIs come from [`@aave-dao/aave-address-book`](https://github.com/bgd-labs/aave-address-book)**
rather than being transcribed, and topics are derived from them at load rather than pasted — a stale
hash matches nothing, which looks exactly like a quiet chain. Every topic derived this way matches
the catalogue in the analysis, which was extracted independently from the Solidity interfaces. It is
the one catalog entry pinned exactly rather than caret-ranged: it is a data dependency republished
most days, so a range would let the addresses the indexer targets change between two installs of the
same commit.

**Storage is append-only. There is no `DELETE` and no mutation anywhere in the package.** The table
is a `VersionedCollapsingMergeTree`, and a reorg retracts rows by writing their negation — the same
mechanism a retry uses, so there is one write path rather than two. Every claim below was measured
against the real engine rather than read off the documentation.

- a retraction pairs on the sorting key, the version and an opposite sign — **not** on the other
  columns, which collapse whatever they hold. Copying them all still matters, and for a sharper
  reason: a materialized view sees each row as it is inserted, so a retraction missing a column
  subtracts from the wrong group. Measured — dropping the user column left `alice=100` standing
  beside `<null>=-100`, a ledger that nets to zero over projections that keep the position. The
  store retracts with `INSERT … SELECT` from the view for that reason, and a spec compares the pair
  column by column;
- two `sign = +1` rows never collapse, so **revert-before-append** is what makes a dispatch
  idempotent, not the engine — the store exposes the two as separate calls so the order is visible
  where it is made, and a spec proves an `append` without the `revert` doubles the range;
- `FINAL` leaks an unpaired retraction row where the `GROUP BY` view does not, which is why reads go
  through `spoke_events_current` and never the table;
- grouping by `version` is required, or a reorg's superseded row and its replacement collapse into
  one group and `any()` can return the stale content.

**Why not `ReplacingMergeTree(version, is_deleted)`**, which would make a repeated insert idempotent
on its own and still retract by appending a tombstone. Because it resolves by _highest version wins_,
so a retraction cancels whatever sits at that key rather than the generation it was meant to kill.
Measured, same sequence under both engines — a reorg tombstone arriving after the new branch has
already been indexed:

|                                | after re-index | after a late tombstone |
| ------------------------------ | -------------- | ---------------------- |
| `ReplacingMergeTree`           | `new-branch`   | **0 rows**             |
| `VersionedCollapsingMergeTree` | `new-branch`   | `new-branch`           |

The retraction here is an `INSERT … SELECT` that copies the version of the row it read, so it cannot
cancel a row it never saw and arrival order stops mattering. That is worth more than free
idempotence, because three ordinary things invert the order: `Date.now()` is wall-clock and steps
backwards under NTP, nothing enforces a single writer, and any retry can reorder two writes. Under Replacing each of those is silent permanent loss that reads as "no
events in that block"; under Collapsing they are harmless, and the idempotence gap they leave is
loud, guarded, and pinned by a spec.

**Materialized views over this table get reorgs for free, and must handle the sign themselves.** An
MV is an insert trigger, so it sees each row as written — a retraction arrives as an ordinary insert
carrying `sign = -1`, and `sum(sign * amount)` nets it out. Measured against a `SummingMergeTree`
target: supply 100 → `100`; retract → `0`; re-index the new branch at 60 → `60`, and the target
sheds the dead row entirely on merge. Two traps come with that:

- **A merge on the source does not fire an MV.** Collapsing took `spoke_events` from 3 raw rows to 1
  and the target stayed at 60. The `-1` row _is_ the deletion, so the sign has to be in the
  projection's arithmetic rather than assumed away.
- **The MV must read `spoke_events`, not `spoke_events_current`.** ClickHouse accepts the DDL over a
  plain view and then never fires it — nothing inserts into a view, so there is no trigger and no
  error. Measured: `42` into the table-sourced target, `0` into the view-sourced one, forever.

**What the retraction costs.** It is one `INSERT … SELECT` per dispatched range, and the primary key
`(chain_id, block_number, log_index)` prunes it to the granules the range touches, so the cost
follows the range rather than the table:

| case                                                  | rows read           | elapsed |
| ----------------------------------------------------- | ------------------- | ------- |
| empty range — every backfill chunk, every quiet block | 0                   | ~10 ms  |
| populated 1000-block range, 57k-row table             | 8,192 (1 granule)   | 35 ms   |
| populated 1000-block range, 627k-row table (11×)      | 15,510 (2 granules) | 10 ms   |

So a full backfill spends well under a second on retractions across all 94 chunks, and at the tip it
is ~10 ms once per block against a 12-second block time. The real cost is write amplification: a
retracted range holds both generations until the next merge — 57,000 rows became 57,056 after
retracting 56 — which is why the reads go through the view rather than waiting for a merge.

**One row shape for all eight events.** Only three of them share a field layout — `Supply`,
`Withdraw` and `Borrow` are identical but for field names — so a column per field would be mostly
nulls and would need a migration per new event. Instead the indexed parameters stay the topic words
they arrived as, in `topic1`/`topic2`/`topic3`, and everything decoded goes in `body` as JSON.

Only `topic1` means the same thing across all eight: the reserve id, or the collateral one on
`LiquidationCall`. `topic2` is `caller`, `user`, `debtReserveId` or `assetId` depending on the event,
and `topic3` is `user` or `hub`, or absent on `ReportDeficit`. A reader that ignores `event_name`
therefore gets the wrong field rather than nothing — which is why **this table is a ledger and
nothing queries it directly**; the position fold reads one view per event, and that is where
`user` becomes a name again.

Every decoded `uint256` is stored as a string: §7.5 is about exactly the habit of narrowing one
because it looks small. The raw topic words and `data` sit beside the decoded `body`, so a decoder
bug is repairable by re-decoding what is already stored rather than re-fetching the backfill —
§9.4's "repair is replay, not patch", one level down.

Verified end to end against mainnet: the same block range was dispatched twice across a restart and
the table holds one generation of it — 107 live rows, zero duplicate keys, zero mutations.

### The Hub ledger, and why it is a second table

Shares are not balances. Debt grows between events because the Hub's `drawnIndex` accrues with
_time_, and that accrual emits no log — so `Supply.amount − Withdraw.amount` is net principal flow
and nothing more (§5). Converting needs Hub asset state, and §5.3 is what makes that cheap: the Hub
emits its own **settled** interest index on every accrual, so the checkpoint is handed over rather
than derived, and no rate-strategy model is needed anywhere.

**Thirteen events**, ten that move additive asset state (`Add`, `Remove`, `Draw`, `Restore`,
`MintFeeShares`, `Sweep`, `Reclaim`, `ReportDeficit`, `EliminateDeficit`, `RefreshPremium`) and three
that set one latest-wins (`UpdateAsset` for the index, `UpdateAssetConfig` for `liquidityFee`,
`AddAsset` for `underlying` and `decimals` — the only source of either, on any contract).

`TransferShares` is excluded on evidence rather than by omission: §4.4 shows it moves `addedShares`
between two `SpokeData` records, so asset-level totals net to zero. Folding it would double-count a
rebalance.

**A separate `hub_events` table, and this is a correctness requirement.** `ReportDeficit` exists on
both contracts with different signatures (§4.4). The position projections filter on `event_name`
alone — they have no address predicate and cannot get one, since the Spoke address is configuration
and a migration does not know it. So a Hub `ReportDeficit` in `spoke_events` fires a view that
reaches for a `user` the Hub form does not have. Measured, once, while deciding this:

| after inserting a Hub `ReportDeficit` into `spoke_events` |           |
| --------------------------------------------------------- | --------- |
| the insert                                                | **fails** |
| rows in `spoke_events`                                    | **1**     |
| rows in `user_positions`                                  | **0**     |

The ledger row commits and the projection row does not, so ingestion jams on a range that will never
succeed — and if the view is later fixed, the next dispatch retracts a row the projection never
received, leaving the position short by exactly that event. `topic1` settles it independently: it is
the **asset id** here and a reserve id there, and one column cannot hold two meanings and stay
queryable.

What the two ledgers do share is the write path. `ClickHouseSpokeEventStore` and
`ClickHouseHubEventStore` are subclasses over one base whose only abstract members are the table and
the view — so the retract-then-append discipline, the full-column retraction and the version
handling are written once. The decoder splits the same way, and the address check lives in the base
where a subclass cannot opt out of it.

**Both ABIs are checked twice.** Every Hub topic0 is derived from `IHubV4_ABI` at load and asserted
against the twelve the analysis extracted independently from the Solidity interfaces. Two
derivations agreeing is the evidence; a hardcoded hash that drifts just matches nothing, which looks
exactly like a quiet chain.

**The backfill floor moved to the Hub's genesis.** Measured: the Core Hub's first log is at
24,720,891, eight blocks _before_ the Main Spoke's 24,720,899, and its first state event is at
24,722,784 — where all 17 `AddAsset` fire in one block. Those eight blocks happen to hold only the
proxy's own lifecycle events, so the old floor lost nothing; depending on that coincidence is the
problem, since `AddAsset` fires once per asset and missing it is indistinguishable from a quiet
chain.

Full history backfilled and counted — genesis to 25,666,327, **58,878 rows across 17 assets**:

| event               | rows   | share      |
| ------------------- | ------ | ---------- |
| `UpdateAsset`       | 29,482 | **50.1 %** |
| `Add`               | 12,249 | 20.8 %     |
| `Draw`              | 7,859  | 13.3 %     |
| `Remove`            | 5,327  | 9.0 %      |
| `Restore`           | 3,910  | 6.6 %      |
| `UpdateAssetConfig` | 34     | 0.1 %      |
| `AddAsset`          | 17     | 0.0 %      |
| the other six       | **0**  | —          |

§4.4 predicted "half of them `UpdateAsset`" and gets 50.1% over the whole history rather than a
sample. Density averages 624 logs per 10k blocks over all history and 988 in a recent 10k-block
window, either side of the analysis's 868.

**The six cold events have never fired — not in a sample, in the entire history of the contract.**
`Sweep`, `Reclaim`, `MintFeeShares`, `RefreshPremium`, `ReportDeficit` and `EliminateDeficit` are all
at zero across 945,000 blocks, which is the event-side confirmation of §5.4's reading that
`premiumShares`, `premiumOffsetRay`, `deficitRay` and `swept` are zero on all 34 assets. It also sets
the ceiling on what the next increment can prove: the asset fold will have seven transitions
reconcilable against mainnet and six that only synthetic fixtures can reach. They are decoded and
pinned by specs here so that the day one fires, it is folded rather than discovered.

Dispatch stays idempotent across the two ledgers, and independently: re-running a 10,000-block range
left the live count unchanged at 1,073, and a Hub retraction over blocks both streams occupy removed
the Hub rows and left the Spoke rows standing.

## The position fold

`@aave-positions/positions` turns that ledger into balances. It contains no ingestion code at all —
the projection is materialized views, so the database maintains it, and a reorg repairs it without
anything in this package hearing about the reorg.

**A position is keyed `(chain_id, user, spoke, reserve_id)`**, §12.1's identity. `reserve_id` is the
one field that means the same thing on all eight events, and a reserve is Spoke-scoped: the same
underlying on two Spokes is two positions with independent risk config and independent health factors
(§12.3), so the Spoke is part of the key rather than a filter. The sorting key leads with `user`,
where the ledger's leads with the block — different access pattern, different table.

Not keyed on the token address, because **no Spoke event carries one**. `AddReserve(reserveId,
assetId, hub)` indexes all three of its parameters and has no `underlying`; the ERC-20 address is on
the Hub's `AddAsset`. So `reserve_id` is the only asset identity a position has here, and both the
Hub's `assetId` and the token address arrive with Hub ingestion.

`AddReserve` is therefore the one ingested event with **no projection**. It is captured in the ledger
like the other seven, so building the `reserveId → (assetId, hub)` registry later reads data that is
already stored rather than re-indexing the chain — but nothing can use `assetId` until there is Hub
state to join it against, and a projection built ahead of its first reader is one more thing to keep
correct for nobody.

### One table per kind of aggregate

Retraction propagates for free to any aggregate that is **a group under addition**. The projection of
a `sign = -1` row is the negation of the projection of its `+1` twin, so sums cancel with no
coordination and no ordering assumption — which is the whole reason idempotence and reorg repair need
no code here. Every share, every amount and the event count are such aggregates, and they live
pre-aggregated in `user_positions`, a `SummingMergeTree` with no `sign` column at all: the values
arrive already sign-multiplied.

`using_as_collateral` is not additive, and **no engine holds a latest-wins fact pre-aggregated under
retraction.** All three candidates were measured rather than argued away:

| approach                                  | what it does when the latest flag is retracted                                             |
| ----------------------------------------- | ------------------------------------------------------------------------------------------ |
| `AggregatingMergeTree(argMaxState)`       | no operation removes a contribution — the `-1` reinforces the state                        |
| `ReplacingMergeTree(version, is_deleted)` | the tombstone deletes the **key**, so the position loses its flag rather than falling back |
| `argMaxIf(flag, (block, log), sign = 1)`  | returns the retracted event — measured `0` where the answer is `1`                         |

The third is the interesting one: a retraction is a _pair_, and the `+1` twin still carries
`sign = 1`. **Liveness is a property of the `(key, version)` group, not of a row** — which is exactly
what `GROUP BY … version HAVING sum(sign) > 0` establishes. So the flag stays at event grain in
`user_position_flags`, collapsing exactly as the ledger does, and is `argMax`ed at read time over
what survives.

**And `version` is not the ordering to `argMax` on.** Two orderings are in play: `version` orders
_dispatches_ (`Date.now()`, stamped per batch, whose job is pairing), while `(block_number,
log_index)` orders _chain events_, which is what "the current flag" means. Ordering by version fails
twice over, both measured — without the collapse it keeps the retracted event anyway, since the
`(+1, −1)` pair sits at the highest version; and with the collapse it still reads the stale flag as
soon as a range is re-dispatched out of block order, which the loop does whenever a later processor
asks to retry.

### One view per event

| view                   | position                | shares                          | amount               |
| ---------------------- | ----------------------- | ------------------------------- | -------------------- |
| `Supply`               | `user`, `reserveId`     | `+suppliedShares`               | `+suppliedAmount`    |
| `Withdraw`             | `user`, `reserveId`     | `−withdrawnShares`              | `−withdrawnAmount`   |
| `Borrow`               | `user`, `reserveId`     | `+drawnShares`                  | `+drawnAmount`       |
| `Repay`                | `user`, `reserveId`     | `−drawnShares`, `+premiumDelta` | `−totalAmountRepaid` |
| `ReportDeficit`        | `user`, `reserveId`     | `−drawnShares`, `+premiumDelta` | —                    |
| `LiquidationCall` ×3   | see below               |                                 |                      |
| `SetUsingAsCollateral` | → `user_position_flags` |                                 |                      |
| `AddReserve`           | no projection, above    |                                 |                      |

**Why the four hot events are not one view with a `multiIf`.** They share a field _layout_ but not
field _names_ — `suppliedShares`, `withdrawnShares`, `drawnShares` — so each branch would name a
different JSON key. `multiIf` does not short-circuit, even with `short_circuit_function_evaluation`
at its default `enable`: measured, a `Supply` row evaluates the `Withdraw` branch, hits a missing key
and throws. Merging therefore forces `toInt256OrZero`, which silently writes `0` for a body it cannot
read — the exact `JSONExtractInt` failure this design rejects.

It is also slower, for the same reason. Every branch runs for every row, so one merged view does four
JSON parses per column where a filtered view does one:

|                           | median, 22,400 mixed events |
| ------------------------- | --------------------------- |
| four views, one per event | **42 ms**                   |
| one merged view           | **64 ms**                   |
| no projections at all     | 15 ms                       |

Filtering on a `LowCardinality(String)` costs less than the extractions it skips, so the extra passes
over the inserted block are cheaper than the work they avoid.

**One `LiquidationCall` log affects up to three positions**, so it gets three views rather than one:
the borrower's collateral (`−collateralSharesLiquidated` on `collateralReserveId`), the borrower's
debt (`−drawnSharesLiquidated` on `debtReserveId`), and the liquidator's collateral
(`+collateralSharesToLiquidator`, only when `receiveShares`). That third leg is §4.1's trap: the
collateral never leaves the Hub, so the liquidator's position grows with **no `Supply` event anywhere
in the trace**, and crediting supplied shares only on `Supply` silently under-counts every
liquidator. It has fired 0 times in 90 mainnet liquidations, which is the argument for folding it now
— it cannot be tested against production data when it first appears.

No leg discriminator column is needed, and that is a consequence of the engine choice: a
self-collateralised liquidation puts the first two legs on the same sorting key, and a
`SummingMergeTree` adds them into one row carrying both deltas. Under a collapsing engine two `+1`
rows at one key would not have been safe.

### Three things measured, one of which changed the design

**`JSONExtractInt` returns `0`, not a truncation.** Above Int64 max it yields zero, and below it
yields the right answer — so it is correct for small positions and silently empties large ones, which
is the worst shape a bug can have. A real fixture `Repay` carries 422,166,581,625,087,607,993, well
past it. Every projection parses `JSONExtractString` → `toInt256`, which throws on anything it cannot
read rather than writing a zero.

**The read view is a `UNION ALL`, not a `JOIN`.** ClickHouse has no index-seek join: every hash
variant reads the entire right side into memory to build a hash table, and `full_sorting_merge` sorts
both sides rather than exploiting that they are already ordered by the join key. A `LEFT JOIN` would
scan, aggregate and hash the whole flag table on every query unless the planner pushed the predicate
into it — and pushdown through a join is the fragile case, where pushdown into `UNION ALL` branches is
not. Concatenating also costs nothing, because `SummingMergeTree` merges lazily and the `GROUP BY` was
required anyway; the flag rows just join an aggregation that was already happening.

**A projection that misses a row cannot be repaired by retrying.** When a materialized view throws,
the insert reports failure but **the ledger row is committed and the projection row is not** —
measured, 1 against 0. The range then jams, because the revert copies the same body back for the
projection to reject again. Worse, if the projection is later fixed, the next dispatch reverts a row
it never received and the position ends up short by exactly that event: measured `0` where the answer
is `100`. Recovery is truncate-and-replay, not a retry and not a merge — the same remedy as the next
paragraph, and a spec pins each step.

**There are no backfill migrations**, for the same reason. A materialized view does not see rows
written before it existed, so adding a projection to a populated ledger needs a replay. The
alternative — a duplicate `INSERT … SELECT` beside each view — can drift from its twin silently,
visible only on a fresh backfill.

**That replay is now an explicit step**, and it used to be free: before the cursor was durable, every
restart re-indexed from `INDEXER_START_BLOCK` and the revert-then-append pass drove every view along
the way. A resuming indexer does not, so adding a projection to a populated ledger means running
[the backfill command](#backfilling-a-range-on-demand) over the range it needs — which re-dispatches
the same processors, and therefore the same revert-then-append, without touching the cursor. That it
moves nothing is what makes it safe to do while the indexer is running.

### Reading it

`PositionStore.list` answers one question: **one wallet's positions on one Spoke.** `user` and `spoke`
are required, not optional filters. Both earn it — together with `chain_id` they are the entire
sorting-key prefix above `reserve_id`, so a page is a seek into contiguous rows rather than a filter
over everyone's; and a Spoke is an isolated margin account with its own collateral factors, oracle and
health factor (§12.3), so two of them are not one list. Cross-wallet questions are analytics over the
same view, not a mode of this port.

Paging is by **keyset, not `OFFSET`**. With the wallet and Spoke pinned, the resume point is a single
`reserve_id`. `OFFSET n` would re-run the aggregation to discard `n` rows, and it shifts under
concurrent writes: a position crossing a page boundary while the indexer advances would be returned
twice or skipped.

**Cursors are HMAC-signed, with the listing mixed into the signature.** Position data is public, so
this is neither confidentiality nor access control — a caller can already ask for any wallet. It buys
two things: the cursor becomes a genuinely opaque contract, so its encoding can change without
breaking anyone who hand-rolled one; and a resume point cannot be carried between listings. That
second one is the real defect a bare signature would leave: unsigned, Alice's cursor is a well-formed
reserve id in Bob's listing, and his first page would silently start past it. Signing
`(chainId, user, spoke)` alongside the reserve id makes switching listings fail the same check as
tampering, and keeps the cursor one field long. HMAC rather than a JWT, which would add a header,
algorithm negotiation and the `alg: none` footgun to protect a single integer. The key is
configuration and **must be identical across replicas** — a per-process secret gives pagination that
fails only under load.

Only open positions come back — §12.1: a position exists while its shares are non-zero. The filter is
`!= 0` rather than `> 0` deliberately, so a negative balance surfaces as a visibly wrong number for
§9 to catch instead of vanishing behind the filter that hides closed positions.

**Balances are shares.** Converting them to assets needs the Hub's interest index and no Hub event is
ingested yet, so `netSuppliedAmount` is the only figure in asset units — and it is a _flow_, not a
balance, because between events the index accrues and emits nothing (§5).

### What it costs

Measured against a full mainnet backfill, genesis 24,720,899 to the tip — 25,729 ledger rows folding
into **5,347 positions, 3,380 of them open**, plus 3,240 flag rows. The event
mix matches the catalogue the analysis extracted independently: 10,181 `Supply`, 5,360 `Borrow`,
4,238 `Withdraw`, 3,240 `SetUsingAsCollateral`, 2,605 `Repay`, 91 `LiquidationCall`, 14 `AddReserve`.

**On the write path**, which is where a materialized view actually charges you, since its SELECT runs
in the inserting thread:

|                                                                  | with the projections | without |
| ---------------------------------------------------------------- | -------------------- | ------- |
| retract a populated 1000-block range (148 events)                | ~23 ms               | ~12 ms  |
| retract an empty range — most backfill chunks, every quiet block | ~8 ms, 0 rows read   | ~8 ms   |
| build the whole fold from the ledger in one pass                 | 215 ms               | —       |

So the projections roughly double a populated retraction and add about 11 ms to it, once per
dispatched range, against a 12-second block time. The empty-range case is unchanged because there is
nothing to project. The last row is the number that matters for the replay rule above: rebuilding
every projection from the ledger is a fifth of a second, not an outage.

**On the read path**, a per-wallet page is ~18 ms and the global "open positions ranked by size" is
~19 ms. Worth being honest about what that does and does not prove: at 5,347 rows the whole
`user_positions` table is a single 8,192-row granule, so a per-wallet read touches all of it no matter
what. The pruning is visible in `EXPLAIN indexes = 1` — both `UNION ALL` branches drop to 1/5 and 2/4
granules — but it cannot show up in rows read until the table outgrows one granule.

**Reconciled against the chain**, which is the check that actually matters (§9): all 3,380 open
positions compared to `getUserPosition` on the Spoke, **zero mismatches**, at zero tolerance (§9.2).
The comparison ran at `latest` rather than a historical block — no archive node — having first
confirmed no in-scope log landed between the fold's last event and the head, which makes the two
states the same state rather than approximately so.

Three paths that reconciliation cannot cover, because mainnet has never exercised them: the premium
triple has never been non-zero, no liquidation has ever set `receiveShares`, and `ReportDeficit` has
never fired. Those are pinned by synthetic specs and nothing else.

## The Hub asset fold

The other half of a balance. A position carries shares; turning them into token amounts needs the
Hub's own asset state, because debt accrues with time through `drawnIndex` and that accrual emits no
log (§5). This folds the Hub ledger into one row per asset, so nothing on the read path needs an RPC.

It **splits along the same axis as the position fold**, and for the same reason. Seven fields are a
group under addition — `liquidity`, `addedShares`, `drawnShares`, `swept`, the premium pair and
`deficitRay` — so retraction propagates for free and they live pre-aggregated in a
`SummingMergeTree`. Seven are latest-wins — `drawnIndex`, `drawnRate`, `realizedFees`, its timestamp,
`liquidityFee`, `underlying`, `decimals` — and no engine holds those pre-aggregated under retraction,
so they stay at event grain in a collapsing table and are `argMax`ed over what the collapse leaves.

### Read from Hub.sol, not from the summary

§5.5 gives a transition table and calls this fold the highest-risk part of the ingestion logic: one
mishandled transition silently corrupts every supply valuation for that asset, with no error and
nothing to flag it. Six of the thirteen events have never fired on mainnet, so reconciliation cannot
catch a mistake in them. Every transition was therefore transcribed from `Hub.sol` at the pinned
commit `2524fe4`. Four things came out of that which the summary alone would have got wrong:

- **`Restore` credits `drawnAmount + premiumAmount` to liquidity**, not `drawnAmount`. The premium is
  cash arriving too. §5.5's table lists only the first, and a fold that believed it would leak the
  premium out of every supplier's redemption value.
- **`liquidity` is assigned, never incremented** — `asset.liquidity = liquidity.toUint120()`. It is
  additive anyway, because the local is `asset.liquidity ± delta` at all six call sites, but the
  `balanceOf` read sitting beside it is a solvency `require` and not the source of the value. Worth
  knowing before folding a field the contract assigns.
- **`EliminateDeficit` and `RefreshPremium` are absent from the summary table entirely.** The first
  does `addedShares -= shares` and `deficitRay -= deficitAmountRay`, falling together so the share
  price is preserved (§12.3); the second applies the same `_applyPremiumDelta` as `Restore`, which
  adds both deltas verbatim — and that is precisely what makes the premium pair additive rather than
  latest-wins.
- **`MintFeeShares` zeroes `realizedFees`**, which would break latest-wins if the zero were never
  emitted. It is: **all 14 functions that call `accrue()` also call `updateDrawnRate()`**, so every
  mutation of `realizedFees` is followed by an `UpdateAsset` in the same transaction carrying the
  settled value, at a higher `log_index`. That invariant is also what explains the event mix — one
  `UpdateAsset` per state-changing call, which is why it is 50.1% of all Hub logs.

`TransferShares` stays excluded, now on read evidence rather than on the analysis's word:
`_transferShares` touches two `SpokeData` records and no `asset.*` field at all.

### Three latest-wins groups, resolved a column at a time

`UpdateAsset`, `UpdateAssetConfig` and `AddAsset` write disjoint columns of the same row and fire at
wildly different rates — 29,482 against 34 against 17 over all history. The newest row for an asset
is almost always an `UpdateAsset` whose `underlying` is NULL, so resolving the row as a whole would
blank the listing fields every twenty seconds. Each column finds its own newest value instead.

The `argMaxIf(col, …, col IS NOT NULL)` is explicitness rather than necessity, and the mutation test
is what established that: swapping in a plain `argMax` changes no result, because **`argMax` already
ignores rows whose argument is NULL** — measured, `argMax(v, ord)` over `(100,'usdc'), (200,NULL),
(300,NULL)` returns `usdc`. The condition stays because it says the intent at the call site and
survives one of those columns ceasing to be nullable.

### Verification

Every guard is mutation-tested — **ten mechanisms removed one at a time, each turning its spec red**:
the `sign` multiply, five transition directions including `Restore`'s premium term and
`EliminateDeficit`'s share burn, the `HAVING` collapse, chain-order-vs-version on the argMax,
`lower()` on the underlying, and the qualified `ORDER BY` (unqualified it binds the `toString` alias
and sorts 13 before 3 — the same bug the position store's pagination hit).

**Reconciled against `getAsset` at zero tolerance**, in the delta form §5.5 itself used: seed from
the chain before the window, replay the window through the fold, compare against the chain after
it. One 59-block window at block 25,667,068, **10 field comparisons across the asset that moved,
zero drift** — exercising `Restore` and the `UpdateAsset` checkpoint.

That window is narrow, and the reason is worth stating rather than hiding: a public endpoint serves
state for about 127 blocks, which bounds the delta form exactly as it bounded §5.5's own 95-block
run. The absolute check — all 17 assets, every field, against a fold holding full history — needs
an archive RPC and is what `reconcile:hub --absolute` does. Everything the narrow window does not
reach rests on the source transcription above, 22 integration specs and the ten mutation tests.

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

- **Position _type_, as §12.1 defines it.** Positions carry the user's own `SetUsingAsCollateral`
  flag, but `collateral` also requires `collateralFactor > 0` under the user's pinned
  `dynamicConfigKey` — and none of the four events that carry it are ingested. Five of the Main
  Spoke's fourteen reserves sit at `CF = 0`, so the flag alone overstates collateral for those. It is
  a real ingestion increment, not a query change: `RefreshAllUserDynamicConfig` alone is the
  highest-volume Spoke event at 8,696 logs, so it roughly doubles the ledger.
- **Valuation on the read path**, which is the last piece before a balance is a number. Both inputs
  are indexed now: `PositionStore` has the shares and `HubAssetStore` has the index. What is missing
  is the `reserveId → (assetId, hub)` registry that joins them — a projection of `AddReserve`, which
  the Spoke ledger already stores — and the §5.1/§5.2 arithmetic. Three traps to respect: rounding is
  _up_ on the debt side and _down_ on the supply side, `premiumRay` is signed so BigInt truncation is
  not `ceil`, and `getDrawnIndex` short-circuits when both share counts are zero. `unrealizedFees` is
  the one formula the analysis summarises rather than states, so it gets transcribed from
  `AssetLogic` at the pinned commit rather than reconstructed.
- **The absolute Hub reconciliation.** `reconcile:hub` runs today in its delta form, which a public
  endpoint's ~127-block state window bounds to one asset or two. `--absolute` compares all 17 across
  every field but needs an archive RPC to have backfilled the fold first. It is the check that
  would catch a mis-folded transition in the six events mainnet has never produced — and it cannot,
  because it can only compare what the chain has actually done.
- **A single-writer guarantee.** The cursor is durable now, so two indexers on one `chain_id` write
  the same row instead of each keeping their own — and a rolling deploy creates exactly that overlap
  while the old pod drains. Bounded damage, but the cursor can move backwards and cost a re-index. A
  `pg_advisory_lock` on `chain_id`, taken at bootstrap and held for the process lifetime, turns the
  second writer into a clean refusal. It is about fifteen lines, and it is its own decision because
  it changes what happens during a deploy: the new pod would refuse to index until the old one
  actually exits.
- **Per-position history.** The fold writes one row per position, not one per event, so "why is this
  number this" is answered by re-deriving from `spoke_events` rather than by reading it back. An
  event-grain table slots in as one more materialized-view target if drift investigation (§9.4)
  becomes routine rather than occasional.
- **The price layer**, and therefore USD values and health factors — ~8 Chainlink aggregators plus
  two LST rebase sources (§7.4).
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
