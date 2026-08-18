# Migration to Rust

The starting point for porting this repository from TypeScript to Rust. It is meant to be the working
document once the work begins, not a memo — the measurements and the gates below are load-bearing.

Its two neighbours: [`design-notes.md`](design-notes.md) is the engineering record of what exists
today, and [`aave-v4-protocol-analysis.md`](aave-v4-protocol-analysis.md) is the protocol groundwork
under both. Neither is superseded by this. The protocol analysis in particular survives the migration
untouched, because it describes Aave rather than this codebase.

## Context

What exists is a working TypeScript indexer and read API for Aave v4: two NestJS services, twelve
workspace packages, ClickHouse for the event ledgers and the folds over them, Postgres for the cursor,
the reorg window and the two enrichment dimensions. It reconciles against the chain at zero tolerance
and has 743 tests behind it. Nothing here is broken; this is a stack decision, not a repair.

So the plan is built around one idea: **the TypeScript implementation is the oracle.** Every phase
ends with a gate comparing Rust output against TypeScript output on the same inputs — same databases,
same block ranges, same HTTP requests. That is a stronger position than the original build had, which
could only check itself against the chain, and the chain cannot answer "did you change behaviour".

**Scope is migration only: no new functionality, no schema changes, no behaviour changes.** The
service stays single-chain, single-Spoke, single-Hub exactly as it is. Anything that alters output
makes the differential gates meaningless, and those gates are the entire safety mechanism.

One thing worth stating before committing to this: **it will not make the indexer faster.** Measured
against the default public endpoint, catch-up runs at ~1.4 ranges/second and is bound by the provider,
not the runtime — roughly 950 ranges and eleven minutes from Main Spoke genesis, and that number is
the same in either language. The real wins are a single static binary, a much smaller image and
resident set, no GC, and `alloy`, which is a better EVM library than viem in precisely the places this
repo has had to work around viem: `fallback` hiding which provider served a call, and the typed
`getLogs` action silently ignoring a `topics` argument.

## Scope, measured

| | source | tests | SQL |
| ---------------------------------------- | -----: | -----: | ----: |
| `apps/api` | 1,547 | 995 | — |
| `apps/indexer` | 2,020 | 304 | — |
| `packages/indexing` | 4,210 | 3,576 | 52 |
| `packages/aave-positions/positions` | 1,458 | 1,832 | 1,117 |
| `packages/aave-positions/events` | 1,156 | 1,345 | 189 |
| `packages/prices` | 901 | 863 | 48 |
| `packages/token-metadata` | 768 | 877 | 58 |
| `packages/ops` | 467 | 296 | — |
| `packages/postgres` | 403 | 330 | 13 |
| `packages/telemetry` | 247 | — | — |
| `packages/clickhouse` | 213 | — | 12 |
| `packages/migrations` | 136 | 156 | — |
| **total** | **14,406** | **10,574** | **1,489** |

**The 1,489 lines of SQL move unchanged.** Schema, materialized views, projections and the read views
_are_ the fold — not application code, and not a line of TypeScript. Roughly a tenth of the system is
already ported before anyone starts.

**The architecture is the second gift.** Seventeen ports already exist as interfaces behind DI
tokens — `ChainClient`, `LogReader`, `Erc20MetadataReader`, `CursorStore`, `BlockHeaderStore`,
`ReorgDetector`, `BlockProcessor`, `EventStore`, `PositionStore`, `HubAssetStore`,
`TokenMetadataStore`, `ReservePriceStore`, `ReservePriceReader`, `SyncStatusStore`, `TokenListings`,
`ReserveListings`, `HealthIndicator`. Each becomes a Rust trait with the same name and the same method
set. The hexagonal shape [the design notes](design-notes.md#layout) argue for is what makes this a
port rather than a rewrite.

## Target layout

A Cargo workspace at the repo root, beside the pnpm one. [`compose.yaml`](../compose.yaml),
[`observability/`](../observability) and every `.sql` file are shared, not duplicated.

```
Cargo.toml                  workspace
crates/
  aave-abi/                 vendored ABI JSON + sol! bindings + addresses   ← new, see Risk 1
  telemetry/                OTLP init for traces, metrics, logs
  ops/                      HealthIndicator trait, probe router, graceful drain
  clickhouse/               client   ← package `clickhouse-client`
  postgres/                 connect
  indexing/                 loop, ports + alloy adapters, reorg, cursor, backfill
  aave-events/              decoders, two append-only ledgers
  aave-positions/           folds, read stores, valuation (the math)
  token-metadata/           enrichment: ERC-20 symbol/name
  prices/                   enrichment: the Spoke oracle
bins/
  migrate/                  refinery, its ClickHouse adapter, the schema,
                            and the only DDL in the system
  api/                      axum
  indexer/                  tokio worker + probe server + five CLIs
```

Thirteen crates against twelve packages. `aave-abi` is the one addition, and exists because of
Risk 1. Two packages have no crate of their own: `packages/migrations` lives inside `bins/migrate`,
because nothing but that binary ever names a `Migration` and nothing can depend on a binary anyway.

**Names are bare, because nothing here is published.** That is the split in the ecosystem: `alloy`,
`reth`, `revm`, `foundry` and `agave` all prefix their package names because crates.io is one flat
namespace, while rust-analyzer and Zed — the two largest unpublished workspaces — use bare `hir`,
`ide`, `editor`, `project` under a workspace-level `publish = false`. Directory names are bare in
every one of them, published or not, which is what the block above already draws. No crate declares a
`version` either, for the same reason: it is optional since Cargo 1.75, and a number nobody publishes
is a number nobody maintains.

The single exception is `crates/clickhouse`, whose package is **`clickhouse-client`**. A member
sharing a name with a dependency makes `cargo -p <name>` ambiguous
([cargo#12891](https://github.com/rust-lang/cargo/issues/12891), open since 2023), and that crate is a
client factory and a probe rather than "ClickHouse" anyway. `crates/postgres` needs no such dodge: the
driver here is `tokio-postgres`, not `postgres`.

**Each binary is a boundary, not just an entry point.** In Node the image is the artifact and
`migrate.js` ships beside `main.js` in a tree where viem is one `require` away, so "the migration tool
cannot reach the chain" is a claim about discipline. Here the dependency graph decides it at link
time:

| binary    | links                                                                | **cannot** link                       |
| --------- | -------------------------------------------------------------------- | ------------------------------------- |
| `migrate` | `clickhouse-client`, `postgres`, and the schema it embeds           | `alloy`, `axum` — no chain, no socket |
| `api`     | `axum`, the read stores, `aave-positions` valuation                  | `alloy`, `indexing`, the write paths  |
| `indexer` | `alloy`, `indexing`, the event and position writers                  | `axum` beyond the probe router        |

The third column is asserted in CI with `cargo tree -i`, so reaching across fails the build rather
than passing review. `migrate` is its own crate because its lifecycle differs — it runs before the
service exists, issues the only DDL in the system, and something has to block on it, which is already
how [`compose.yaml`](../compose.yaml) treats it. It also owns every migration concept in the
workspace: the database crates it uses are connectivity and nothing more, so a crate named for a
database is never where migration semantics are decided. The five operational CLIs need the indexer's
graph exactly, so they stay `[[bin]]` targets of `bins/indexer`.

## Dependency mapping

| today | Rust | note |
| --- | --- | --- |
| NestJS DI + modules | explicit composition in `main.rs` | 32 `@Injectable`, 43 `forRootAsync` and 55 `@Inject` all disappear. Ports become `Arc<dyn Trait + Send + Sync>` — trait objects rather than generics, so the composition root reads like the module graph it replaces |
| `@nestjs/platform-express` | `axum` + `tower-http` | |
| `@nestjs/swagger` | `utoipa` + `utoipa-swagger-ui` | derive-based, so it maps onto the hand-decorated DTOs directly; the OpenAPI drift guard ports as a test over the generated document |
| `zod` | `serde` + `garde`; `figment` for env | five files. Abort-on-invalid-config is preserved |
| `viem` | `alloy` | `alloy-provider`, `alloy-sol-types`, `alloy-transport-http` |
| `@clickhouse/client` | `clickhouse` crate | 19 call sites, all `JSONEachRow` today; inserts move to RowBinary and `JSONEachRow` stays only where a `body` column is genuinely JSON |
| `postgres` (postgres.js) | `tokio-postgres` + `deadpool-postgres` | see [Port notes](#port-notes) |
| `pino` + `nestjs-pino` | `tracing` + `tracing-subscriber` JSON | one JSON object per line is preserved |
| OpenTelemetry JS SDK | `opentelemetry` + `opentelemetry-otlp` + `tracing-opentelemetry` | see Risk 2 |
| `bigint` | `alloy_primitives::U256` / `I256` (ruint) | see [Port notes](#port-notes) |
| `node:crypto` HMAC | `hmac` + `sha2` + `subtle` | cursor signing; `subtle` for the constant-time compare |
| `vitest` | `cargo test` | store specs keep running against real servers, reached through `CLICKHOUSE_URL` / `POSTGRES_URL` exactly as the vitest configs reach them, with CI providing both as service containers |
| `supertest` | `tower::ServiceExt::oneshot` | no socket needed |
| `oxlint` + `prettier` | `clippy` + `rustfmt` | keep [`lefthook`](../lefthook.yml), add both |

## Risks

Three things here are genuinely uncertain. Everything else is work, not risk.

### 1. `@aave-dao/aave-address-book` is JavaScript-only

Three files import ABIs from it and two import addresses. It is pinned to `4.61.2` deliberately — the
[catalog comment](../pnpm-workspace.yaml) explains that it is a data dependency republished most days,
so a caret range would let the addresses the indexer targets change between two installs of the same
commit. There is no Rust equivalent of it.

`crates/aave-abi` holds the extracted `ISpokeV4`, `IHubV4` and `IAaveOracleV4` JSON with
`alloy::sol!` generating the bindings, plus the Main Spoke, Core Hub and oracle addresses as
constants — the same single-Spoke, single-Hub configuration as today. **A CI job re-extracts from the
pinned npm version and fails on any diff.** That keeps the design notes' claim that the ABI is "taken
from the official address book rather than transcribed" true, and turns an upstream change into a red
build rather than a silent divergence.

### 2. There is no auto-instrumentation in Rust

The TypeScript build gets HTTP, undici, NestJS and pino spans from four `registerInstrumentations`
entries. Rust has no equivalent: `tower-http::trace` gives inbound HTTP, `tokio-postgres` gives
Postgres, and everything else — the ClickHouse client, every alloy RPC call — is instrumented by hand.

The span names and attributes are already written down in
[Tracing and metrics](design-notes.md#tracing-and-metrics), and all sixteen metric names are fixed and
verified against a live Prometheus. Port them as an explicit inventory and assert on it: a test that
force-flushes an in-memory exporter and checks the exact set of instrument names, so a missing span
fails CI rather than a dashboard. Provider health gets _easier_ — alloy's transport layers offer a
proper hook where viem needed `fetchFn`.

### 3. Each cutover destroys its own oracle

The whole safety argument rests on diffing Rust against TypeScript. The moment `apps/api` is deleted
that comparison is gone for the API, and the same again for the indexer. Any bug not caught before a
deletion is caught by nothing afterwards.

The mitigation is procedural, which is exactly why it is a risk rather than a step: **the replay
corpus is captured and committed before the deletion PR, not after**, and the deletion PR is separate
from the one that adds the Rust service so the two revert independently. Soak with both running
first.

## Port notes

Settled decisions, recorded so they are not relitigated mid-migration.

### The arithmetic

[`ray.ts`](../packages/aave-positions/positions/src/valuation/ray.ts) is transcribed from
`aave/aave-v4` at commit `2524fe4` and reconciles at zero tolerance, so the Rust keeps the same shape:
one function per Solidity function, same rounding, same revert conditions. Three specifics.

`premiumOffsetRay` is `int200` on chain and genuinely negative, so `premium_ray` takes **`I256`** — the
only signed value in the module.

`a * b / c` goes through a `mul_div_down` / `mul_div_up` helper built on
[`ruint::widening_mul`](https://docs.rs/ruint/latest/ruint/struct.Uint.html), which is what Solidity's
`Math.mulDiv` achieves with a 512-bit intermediate and what the
[Uniswap V3 Rust ports](https://github.com/0xKitsune/uniswap-v3-math) do. Adding
`#![deny(clippy::arithmetic_side_effects)]` to the crate makes that mechanical rather than a matter of
discipline.

And **not** `(a / c) * b`. Integer division truncates the fractional part, and in fixed-point the
fractional part _is_ the value: measured on a real `drawnIndexAt` — `a` =
1008055395294113139752655573, `b` ≈ RAY — that form comes out **0.7991% low**, which would report
near-zero interest on every position. The remainder-corrected variant
`(a / c) * b + ((a % c) * b) / c` is exact, verified bit-identical on the same inputs, and is a
legitimate way to avoid a wide intermediate; it is simply not needed here.

Measured headroom, because nothing currently records it:

| intermediate | width | headroom |
| --- | ---: | ---: |
| `rayMulUp(index, linearInterest(…))` — ray × ray | 180 bits | 76 |
| `drawnShares * drawnIndex`, in `aggregatedOwedRay` — uint120 × ray | 210 bits | 46 |
| `mulDivDown(shares, totalAssets, totalShares)` | 248 bits | **8** |

Nothing overflows on realistic values, and the RAY-scaled aggregate never feeds another multiply —
`aggregatedOwedRay` is consumed by `fromRayUp`, which divides. Write those bounds into the doc
comments anyway. The TypeScript never had to carry them because `BigInt` has none, which is the whole
reason they are worth stating on the way across.

### The Postgres driver: `tokio-postgres` + `deadpool-postgres`, not `sqlx`

Rust needs a driver and a pool, which is exactly the role `postgres.js` fills today. `sqlx` is the
popular choice and its headline feature — verifying SQL against a live database at compile time —
means either a running Postgres during `cargo build` or a committed cache to keep in step. That is a
real build-system cost for little gain here: the queries are few, each lives in one adapter, and they
are already covered by store specs that run against a real server.

Parameters stay bound (`$1`, `$2`), so the parameterisation property the catalog comment praises
holds, and the only unbound string remains the DDL in the migration runner, which is a repo file.
`tracing` spans come natively, which means
[`traced-sql.ts`](../packages/postgres/src/traced-sql.ts) — the `Proxy` over `Sql` with the shadowed
`then` — is **deleted rather than ported.**

### Config ordering

`OTEL_*` is read by the preloaded SDK before Nest exists, which is why
[Configuration](design-notes.md#configuration) carries a paragraph explaining that one group of
variables does not go through Zod. In Rust `main()` initialises telemetry and then the app, from one
parsed config. Delete the explanation along with the problem.

## Phases

Each is a PR, and each ends with a gate run against the TypeScript implementation. **Cutover is per
service** — the Rust API goes to production and the TypeScript API is deleted before indexer work
finishes, rather than one switch at the end.

### Phase 0 — foundations

Cargo workspace, `aave-abi` and its ABI drift job, `telemetry`, `ops`, `clickhouse-client`,
`postgres`, and `bins/migrate`.

**The runner is [refinery](https://docs.rs/refinery), not hand-rolled.** It drives Postgres natively.
ClickHouse is not one of its backends, but adding one is three trait impls — `AsyncTransaction`,
`AsyncQuery<Vec<Migration>>` and one `AsyncMigrate` method overriding the ledger DDL, because
refinery's default is `VARCHAR(255)` with an `int4 PRIMARY KEY` and ClickHouse has neither. That is 42
lines against the 169 a second hand-rolled runner would have cost, and fourteen of the eighteen
migrations are ClickHouse, so the alternative was refinery for four files and hand-rolled for the rest.

Two things follow. The ledger becomes refinery's — `refinery_schema_history`, keyed on an integer
version — so the ids are relabelled `V12__position_projections` where the file stays
`012_position_projections.sql`; `schema.rs` carries both names. And the statement splitter survives:
refinery hands the adapter a migration's whole SQL as one query, and ClickHouse's HTTP interface
refuses multi-statement bodies, so it is taken apart inside `AsyncTransaction::execute`.

**The `.sql` files terminate their statements with `;`, and the splitter is a lexer.** They were
separated by a `--@statement` comment marker, which meant a multi-statement file could not be pasted
into a console at all: the client parses the whole buffer as one query and fails at the second
statement, creating nothing — verified against clickhouse-client, 0 of 9 objects. A migration you
cannot paste is one you cannot debug when it matters. Splitting on the character is therefore not a
`find(';')`: 723 `--` comments, 184 string literals and 215 backtick identifiers across the corpus
hold nineteen semicolons between them, all in prose. The scan tracks what it is inside; only a
semicolon in open code terminates.

**The `.sql` files are embedded, not discovered.** The TypeScript reads its migration directories with
`readdir` at startup, which only works because `pnpm deploy` copies them next to the compiled output;
a binary shipped on its own has nothing to read. Every Rust migration tool embeds at compile time —
`sqlx::migrate!`, `diesel_migrations`, `refinery` — and the reason diesel gives is precisely this one,
that it is what lets you ship a single executable. So each crate that owns tables declares a
`const MIGRATIONS: &[Migration]` with **one `include_str!` per file, written by hand**: not
`include_dir!`, whose proc macro cannot register a rebuild dependency on stable and would happily ship
yesterday's SQL from a warm `target/`, and not a globbing macro either, since adding a file would then
change no Rust source and the macro would not re-run
([sqlx#681](https://github.com/launchbadge/sqlx/issues/681) is the reference for both halves).

Two consequences. What the directory read was buying — nobody has to remember to register a file — is
bought back by `check_complete`, a test that compares the constant against the directory in both
directions.

And the ordinal check changes shape, because it is no longer answering the same question. In
TypeScript the set was discovered and sorted, so two migrations sharing an ordinal left the apply
order to whichever array the caller concatenated first; the guard existed to reject that ambiguity, at
run time, because the list was only known once the process was up. Here the array *is* the apply
order, so there is no ambiguity — and no runtime, since a constant cannot change after it is compiled.
What is left worth enforcing is that the array agrees with the directory listing a reader sees, so
`check_order` asserts **strictly ascending ordinals**, once per database union, as a test. Verified
against the real corpus: all twenty files are already ascending in the order the application
concatenates them.

**Gate:** both migrators applied to empty databases produce the same schema. The ledger tables are
excluded and expected to differ — that is the point of adopting refinery — so what is compared is
`SHOW CREATE TABLE` for every other object and `pg_dump --schema-only`. Measured: 35 ClickHouse
objects and 34,367 bytes identical, 59 lines of Postgres schema identical, and a second run reporting
"schema already up to date".

Split in two. **2a** is the two runner crates alone, proven against live servers — including the fact
that their failure semantics differ, which is the reason there are two: a set that fails partway
rolls back entirely on Postgres and leaves what already succeeded on ClickHouse. **2b** adds
`bins/migrate`, the eighteen `include_str!` constants and the byte-identical comparison above.

Phase 0 took two PRs. The first stood up the workspace and the statement splitter, gated on a
differential over the twenty real `.sql` files against the TypeScript loader. The second added the
two database crates and `bins/migrate`, gated on the byte-identical comparison above.

### Phase 1 — the arithmetic

`aave-positions::valuation` and `::ray` only. No storage, no wiring.

**Gate:** a `proptest` differential harness driving the TypeScript implementation — a thin `node`
process over stdin/stdout — and the Rust one over randomised inputs across the whole domain,
asserting bit-identical output _and_ identical error conditions, including the deliberate throws on a
negative premium and on a checkpoint ahead of the valuation time. Seed it to search for the overflow
boundary rather than assume it is distant: inputs at and beyond `uint120` shares and 2^128
`totalAssets`, asserting `Err` exactly where the widened arithmetic says it must come. Plus the 36/36
chain reconciliation reproduced in Rust.

This phase must not be rushed; everything downstream inherits it.

### Phase 2 — the read API, then ship it

`bins/api` on axum, the `PositionStore` ClickHouse adapter, `HubAssetStore`, the read halves of
`token-metadata` and `prices`, cursor signing, request validation, utoipa.

**Gate, and it has to happen before anything is deleted:** both APIs pointed at the same ClickHouse
and Postgres, and a replay harness issuing several hundred requests — every wallet in the fold, every
page size, cursors walked to exhaustion, `asOf` pinned at fixed instants — **byte-comparing the
JSON**. Diff the two OpenAPI documents too.

**Then capture that corpus as golden files and commit it.** Once `apps/api` is gone the oracle is
gone, so the recorded request/response pairs become the regression suite that replaces it. Deploy the
Rust API, soak, then delete `apps/api` in its own PR. `packages/*` stay — the TypeScript indexer still
needs them.

### Phase 3 — the indexing engine and Aave ingestion

`indexing` — the loop, `ReorgDetector`, `CursorStore`, `BlockHeaderStore`, backfill, the alloy
adapters and provider health — and `aave-events`, the decoders, both ledgers and the processors. The
port contract suites in `test-support/` port with them; they are what keeps the in-memory doubles
honest.

**Gate:** the Rust indexer walks from Main Spoke genesis into a _fresh_ pair of databases while the
TypeScript one walks into the existing pair, then `SELECT`-level diffs of `spoke_events_current`,
`hub_events_current`, `user_positions_current` and `hub_assets_current` at the same block. Zero rows
of difference. Plus the reorg harness driven through every window shape the loop can leave behind.

### Phase 4 — enrichment, prices, the CLIs, then ship the indexer

The `token-metadata` and `prices` write paths, then `backfill`, `migrate`, `reconcile:hub`,
`reconcile:positions`, `enrich:tokens` and `price:reserves`.

**Gate:** Rust `reconcile:hub` and `reconcile:positions` report zero drift against the fold the
_TypeScript_ indexer produced. That is the strongest cross-check available anywhere in this migration,
because each half was built by a different implementation. Then deploy the Rust indexer, soak against
the live chain, and delete `apps/indexer`.

### Phase 5 — remove the TypeScript

Delete the remaining `packages/*`, `pnpm-workspace.yaml`, the Node toolchain and the pnpm CI job.
Point [`compose.yaml`](../compose.yaml) fully at the Rust images. Rewrite the design notes'
implementation-specific sections; the protocol analysis is untouched.

## Verification

Beyond the per-phase gates:

- **Equal test scope, not an equal count.** Every behaviour the 743 specs pin gets a Rust counterpart,
  but idiomatic Rust will merge some cases into table tests and split others, so the number will move.
  What must not shrink is the coverage — in particular the port contract suites, the
  [four ERC-20 hazards](design-notes.md#four-erc-20-hazards-each-measured), the collapsing-semantics
  store specs, and the reorg window shapes. Store specs keep running against real ClickHouse and
  Postgres, because what they assert is that the SQL executes.
- **The mutation tests survive the move.** The design notes name specific mutants that must turn a
  spec red — dropping the `context.with()`, the error status on a retry, the head clamp, the failover
  transition guard, awaiting inside `onBlockRange`, collapsing the two enrichment outcomes into one.
  Re-run each against the Rust port; a surviving mutant means the ported spec is weaker than the
  original.
- **The whole-stack gate**, unchanged: a cold `docker compose up`, one real request, and the trace,
  the log lines carrying its `trace_id`, and its effect on the dashboard all present in Grafana.
- **Overhead re-measured** both ways, so the README's numbers describe the thing that ships.

## Deliberately not in this migration

**No new functionality of any kind.** Not multi-Spoke or multi-Hub, not `interestEarned` /
`interestPaid` fields, not per-Spoke net worth, not health factors, not continuous reconciliation. Not
a schema change and not a performance change. Every one of them would invalidate a differential gate,
and the gates are the only thing that makes a 25,000-line port safe. They land after cutover, against
a Rust codebase already proven identical to the one it replaced.
[Not here yet](design-notes.md#not-here-yet) keeps the list.

## References

The arithmetic decisions rest on these rather than on recall:
[uniswap-v3-math](https://github.com/0xKitsune/uniswap-v3-math) ·
[ruint::Uint](https://docs.rs/ruint/latest/ruint/struct.Uint.html) ·
[primitive-types::U256](https://docs.rs/primitive-types/latest/primitive_types/struct.U256.html) ·
[spl-math](https://docs.rs/spl-math/latest/spl_math/) ·
[Uniswap FullMath](https://docs.uniswap.org/contracts/v3/reference/core/libraries/FullMath)
