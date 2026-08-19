# aave-v4-positions

[![CI](https://github.com/EugeneButusov/aave-v4-positions/actions/workflows/ci.yml/badge.svg)](https://github.com/EugeneButusov/aave-v4-positions/actions/workflows/ci.yml)

An indexer and read API for **Aave v4 user positions**. It walks Ethereum mainnet, decodes Spoke and
Hub events into append-only ledgers, folds those into per-wallet balances, enriches them with two
things no Aave event carries — what each token calls itself and what Aave prices it at — and serves
the result over a paged HTTP API.

Two documents sit under this one, and they are where the reasoning lives:

- **[Design notes](docs/design-notes.md)** — the engineering record. Every decision that was not
  obvious, what was measured to settle it, and what each piece costs.
- **[Protocol analysis](docs/aave-v4-protocol-analysis.md)** — the groundwork under both: where
  position state actually lives in v4, which events reconstruct it, the share/asset arithmetic, and
  the health-factor and price layers.

## Why v4

**v4 rather than v3 because it is new** and Hub-and-Spoke makes it a different indexing problem
rather than a familiar one — position state and the index that values it live on different
contracts, no Spoke event names a token, and nothing may be summed across Spokes. Those three
constraints, not preference, are what the schema, the two ledgers and the response shape below are
built around.

Since v3 intuition is the wrong prior here, every protocol claim this rests on was read out of the
contracts and marked `[verified]` in the [analysis](docs/aave-v4-protocol-analysis.md) rather than
assumed.

## Scope

### What it does

- **Indexes** the eight Main Spoke events that move a position and the thirteen Core Hub events that
  value them, decoded against the official ABIs into two append-only ClickHouse ledgers.
- **Folds** both into read models the database maintains: per-wallet positions, and a per-asset
  mirror of Hub state reconciled against the chain's own `getAsset`.
- **Enriches** from two sources outside the event log — ERC-20 `symbol()`/`name()` read off the
  token contracts, and USD prices read from the Spoke's own Aave oracle — both stored in Postgres,
  both kept current without anyone running a command.
- **Serves** it at `GET /api/v1/chains/{chainId}/users/{user}/positions`, keyset-paged, block-stamped
  and documented at `/docs`.
- **Stays correct while the chain moves**: incremental sync from a durable cursor, an on-demand
  backfill command, and reorg detection over a retained hash chain that repairs the ledger by
  retraction, with the folds collapsing it away.
- **Is shaped for Kubernetes**: split liveness/readiness probes, readiness-first drain, configuration
  that aborts the process rather than defaulting, one JSON object per log line, and OpenTelemetry
  traces, metrics and logs over OTLP with a Grafana that `docker compose up` brings up provisioned.

### What is not here

Two things are worked out in the analysis and deliberately not built here, for one reason: **each
wants a data-modelling increment rather than an endpoint**, so the cut-down version of either would
have been a number nothing in this repository could stand behind.

**Per-Spoke totals and net worth.** Built and then parked
([#25](https://github.com/EugeneButusov/aave-v4-positions/pull/25)). Totalling on top of a paged read
leaves two options and both are bad: sum the page, which is arithmetic on a subset that looks exactly
like a whole number; or issue a second unpaged read of every position the wallet holds, which is what
that PR did — correct, but it recomputes the same aggregate on every request and needs a refusal
threshold to stay bounded. The aggregate wants **its own representation**, maintained the way the
position fold is maintained rather than derived per request. That is a schema and ingestion change,
so it is its own increment rather than an addition to this one.

**Health factor.** It needs risk parameters this build does not ingest: `collateralFactor` at the
user's pinned `dynamicConfigKey`, which comes from `AddDynamicReserveConfig` /
`UpdateDynamicReserveConfig` together with `RefreshAllUserDynamicConfig` /
`RefreshSingleUserDynamicConfig`. The last of those is the **single highest-volume event on the
Spoke** — 8,696 logs in the sampled window — so ingesting the set roughly doubles the ledger, which
makes it an ingestion increment with its own storage and reconciliation story. The same events gate
the `collateral` position _type_: five of the Main Spoke's fourteen reserves sit at a zero collateral
factor, so the user's own flag alone overstates collateral for those, and the API says so in the
field description instead of implying a guarantee. A historically accurate health factor additionally
wants a price _history_; what exists here is the oracle read at the head on a timer.

Everything else deferred is listed, with reasons, in
[Not here yet](docs/design-notes.md#not-here-yet).

### Assumptions

- **One chain, one Spoke, one Hub** — configured rather than hardcoded. A second Spoke is a second
  registration, not an edit.
- **A full node is sufficient.** Ingestion reads logs; only enrichment calls contracts, and only at
  the head. Nothing on the read path calls a node at all — that is the property both folds exist to
  buy.
- **The RPC endpoint is unreliable**, and is treated that way: an ordered provider list with
  failover, a range size that halves when a provider rejects it, bounded retries, and every request
  attributed to the URL that served it.
- **128 blocks is deep enough for finality.** Anything shallower is unsettled, which is exactly the
  window the reorg detector retains and guards.
- **Dispatch is at-least-once and processors are idempotent.** That is what makes re-running a range
  a defined operation rather than a repair.
- **A token's `symbol()` is a label, not an identity.** Nothing stops two tokens claiming the same
  one; `underlying` is the identity and the contract says so. No anti-spoofing is attempted.
- **The API is a read model.** It writes to neither database, and stamps every response with how far
  the indexer had got.

## Running it

Nothing to install but Docker — no Node, no pnpm:

```bash
docker compose up -d --build
```

That brings up seven services: ClickHouse and Postgres, a one-shot `migrate` that applies both
schemas and exits, the API and the indexer, and the two that watch them. First build is a few
minutes; after that `docker compose ps` shows the long-running five as `healthy`.

|                                     | where                                                                  |
| ----------------------------------- | ---------------------------------------------------------------------- |
| API                                 | <http://localhost:3000>                                                |
| OpenAPI / Swagger UI                | <http://localhost:3000/docs>                                           |
| Indexer probes                      | <http://localhost:3001/health/ready>                                   |
| **Grafana** (traces, metrics, logs) | <http://localhost:3333> — no login, opens on the provisioned dashboard |
| ClickHouse                          | `localhost:8123`                                                       |
| Postgres                            | `localhost:5432`                                                       |

Watch it work:

```bash
docker compose logs -f indexer
```

It indexes 1000-block ranges up from the Main Spoke genesis, then one block at a time at the tip.
Catching up all the way is about **950 ranges as of August 2026** — two `eth_getLogs` each — which
measured at ~1.4 ranges a second against the free public endpoint, so **roughly eleven minutes**, and
it grows by seven or so ranges a day. To skip the history while trying the stack out, start a few
thousand blocks below the tip:

```bash
INDEXER_START_BLOCK=25665000 docker compose up -d --build
```

`INDEXER_START_BLOCK` is read on a **cold start only** — once a cursor row exists it is never
consulted again, so changing it after the fact means a `docker compose down -v` too.

A full run from genesis to block 25,669,898 landed 25,908 Spoke events and 59,178 Hub events, folded
into 5,381 positions, with all 17 listed tokens labelled and 14 reserves priced. Then ask it
something:

```bash
curl -s 'http://localhost:3000/api/v1/chains/1/users/0x82d16ff1c724ab72f218a3f7f6dd3e5385ee87e8/positions?limit=1' | jq
```

Tear down with `docker compose down`, or `down -v` to drop the indexed data and the cursor with it.

**It uses a public endpoint by default** (`https://eth.drpc.org`), so a bare `up` makes real requests
to a third party. Point it at your own node with `RPC_URLS=https://your-node docker compose up -d`,
or set `INDEXER_AUTOSTART=false` for probes only. Ports, chain, start block and everything else are
environment variables — the full table is in
[Configuration](docs/design-notes.md#configuration), and each service's `.env.example` is annotated.

### Backfilling a range

The loop backfills to _reach_ the tip on its own. The command backfills a range you **name**, and
moves nothing — for a processor whose decoding was wrong over known blocks, a processor that joined
after the loop walked past the history it needs, or a range being checked by hand:

```bash
docker compose run --rm indexer node dist/backfill.js --from 24720899 --to 24730899
```

Add `--dry-run` to see what it would do, `--processors 'aave-spoke(0x94e7a5dc)'` to narrow it to
one, and `--help` for the rest. It shares the indexer's configuration and wiring but deliberately
not its state: no cursor, so it cannot move a running indexer's resume point; no reorg handling, so
it refuses a range reaching above the safe head rather than clamping it. Running it against a live
indexer needs no coordination.

Outside Docker it is `pnpm backfill --from … --to …`.

### Without Docker

```bash
pnpm install
cp apps/api/.env.example apps/api/.env && cp apps/indexer/.env.example apps/indexer/.env
cargo run --release -p migrate                     # schema is its own step, never done at boot
pnpm dev:indexer                                   # and, in another shell, pnpm dev:api
```

Needs Node 24, pnpm 11, Rust 1.96 — `rust-toolchain.toml` pins it and rustup installs it on the
first `cargo` command — and a ClickHouse and Postgres to point at;
[Prerequisites](docs/design-notes.md#prerequisites) has the two `docker run` lines. `pnpm check` runs
format, lint, typecheck and the TypeScript suite: **716 tests across 63 files, in about 17 seconds**.
It is not the whole gate any more — CI also runs `cargo fmt`, `clippy`, `cargo test --workspace` and
a release build, so a change under `bins/` or `crates/` passes `pnpm check` and can still fail.
Docker is required for them too — the store specs go against real servers rather than mocks, because
what they assert is that the SQL executes, and no fake can tell you that.

## The API

One endpoint today, and the shape is deliberate:

```
GET /api/v1/chains/{chainId}/users/{user}/positions?spoke=&limit=&cursor=&asOf=
```

A real response, trimmed to one item — the indexed half, the folded half and both enrichments in the
same payload:

```jsonc
{
  "sync": {
    "lastBlock": 25670682,
    "updatedAt": "2026-08-03T00:31:11.779Z",
    "ageSeconds": 2,
    "stale": false,
  },
  "valuedAt": "2026-08-03T00:31:14.000Z",
  "pricing": { "updatedAt": "2026-08-03T00:30:20.627Z", "ageSeconds": 53, "stale": false },
  "items": [
    {
      "chainId": 1,
      "user": "0x82d16ff1c724ab72f218a3f7f6dd3e5385ee87e8",
      "spoke": "0x94e7a5dcbe816e498b89ab752661904e2f56c485",
      "reserveId": "0",
      "suppliedShares": "24.462137244885799974", // indexed: folded from 144 events
      "usingAsCollateral": true,
      "events": 144,
      "asset": {
        "underlying": "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2", // resolved through both ledgers
        "decimals": 18,
        "symbol": "WETH", // enriched: the token's own symbol()
        "name": "Wrapped Ether",
      },
      "value": {
        "suppliedAmount": "24.600772347039817016", // shares × the Hub index at valuedAt
        "totalDebt": "0",
        "drawnIndex": "1.008055395294113139752655573",
        "priceUsd": "1871.4", // enriched: the Spoke's own oracle
        "suppliedAmountUsd": "46037.8853702503135637424",
      },
    },
  ],
  "nextCursor": "MHg5NGU3YTVkY2JlODE2ZTQ5OGI4OWFiNzUyNjYxOTA0ZTJmNTZjNDg1fDA.ypox2aUS9gUn75PeUxZX6w",
}
```

- **Every number is a decimal string, already scaled** — whole tokens, dollars, or ray ratios where
  `1` is no accrual. Nothing on the wire is in base units, and nothing should be parsed into a float:
  a share balance can carry 21 significant digits and a double keeps 17.
- **A field is null when the answer is unknown, never zero.** `asset` and `value` are null together
  when the registry has not resolved a reserve; the `*Usd` fields are null when the oracle has not
  priced it.
- **Three clocks, because there are three sources.** `sync` is how far the indexer got, `valuedAt` is
  the instant the page's amounts were computed for, and `pricing` is how old the oldest price behind
  it is. Collapsing them into one would be a number that was never true.
- **Cursors are opaque and signed**, valid only for the listing that issued them, so changing a
  filter mid-walk is refused rather than silently resuming somewhere else.

Full contract at `/docs`; a drift guard in the test suite fails the build if any operation lacks a
typed response.

## How it is put together

The decisions worth knowing before reading the code. Each links to where it is argued out in full.

**Two databases, and the split is by access pattern.** ClickHouse holds the event ledgers and the
folds over them: append-only, columnar, and read in ranges. Postgres holds everything small, mutable
and point-read — the indexer's cursor, the reorg header window, token labels, prices. The second one
was benchmarked rather than assumed: upserting 17 rows is 2 ms in Postgres against 67 ms in
ClickHouse, and leaves 17 rows at rest instead of 51 waiting to be merged.
[Why](docs/design-notes.md#postgres-and-the-benchmark-that-chose-it).

**The ledger is append-only and the fold is the database's job.** Events are written once; per-wallet
balances are materialized views over them. This is what makes reorg repair cheap: the indexer writes
the _negation_ of the rows a fork invalidated, and `VersionedCollapsingMergeTree` collapses both away
— no projection is told a reorg happened, and no rebuild is scheduled.
[Why](docs/design-notes.md#the-position-fold).

**Reorgs are detected from a hash chain, not from block numbers.** A retained window of headers,
`FINALITY_DEPTH` deep, walked back to the first block whose parent still matches. A detected fork is
re-reported on every start until it has actually been applied, so a crash mid-repair cannot leave
the ledger quietly wrong. [Why](docs/design-notes.md#indexing).

**The indexing engine knows nothing about Aave.** `@packages/indexing` owns the loop, the chain
ports, the cursor seam and the reorg detector; what to _do_ with a block range is a `BlockProcessor`
the application registers. That boundary is why the Aave half is independently testable, why
enrichment could be added as one more processor without touching the loop, and why every seam has an
in-memory double that runs the same contract suite as the real adapter.
[Why](docs/design-notes.md#layout).

**Enrichment is merged in the service, never joined in SQL.** Labels and prices have their own
source, cadence and failure mode, so they are read beside the position query rather than after it,
and composed in `PositionsService`. `Position` and the ClickHouse store do not know either exists —
which is the whole reason a third source is additive rather than a change.
[Why](docs/design-notes.md#enrichment), and
[the shape a third one takes](docs/design-notes.md#adding-another-enrichment-source).

**Nothing on the read path calls a node.** Both folds exist to buy that property, and enrichment does
not spend it: it writes a table the API reads rather than reaching for an RPC while answering a
request.

**Operability is part of the build, not a follow-up.** Probes that mean different things, a drain
that fails readiness before it closes the server, configuration validated at boot, structured logs,
and OpenTelemetry across all three signals — with the response's `x-request-id` being the trace id,
so a bug report leads straight to a span tree and the log lines under it.
[Why](docs/design-notes.md#operational-shape).

## What comes next

**Continuous reconciliation against the chain, not a script someone remembers to run.** This is the
one to do first. Both checks exist and both pass — `reconcile:hub` compares the asset fold to
the Hub's own `getAsset`, and `reconcile:positions` compares valued positions to
`getUserSuppliedAssets` and `getUserDebt`, at zero tolerance, with both sides pinned to one block so
accrual cannot be mistaken for drift. But they are **one-shot CLIs over a list of users you pass in**,
so what the repository can currently say is "drift was zero the last time somebody looked" — and that
is a weaker claim than it sounds, because the failure this guards against is silent by construction:
a mis-folded transition produces plausible numbers, not an error.

What it wants is a scheduled job: a rotating sample of open positions each cycle rather than the whole
book, plus all seventeen Hub assets, publishing a mismatch count as a metric so the signal is
_continuously zero_ instead of _zero on demand_. Two things shape it. Its output is an alert and a
block range, never a mutation — repair here is replay, not patch — and the position half needs full
history folded, so a cold environment needs an archive-capable RPC to backfill before the job means
anything. The design is already written up in §9 of the analysis, including when it should run and
why the tolerance is zero; what is missing is the thing that runs it.

**Enrichment should be jobs, not a processor with a timer.** Today a run is triggered by the
ingestion dispatch, holds newly listed tokens in an in-memory buffer, and backs off on a wall-clock
timer when it leaves a gap open. A durable job queue would make each token its own item with its own
attempts, backoff and visibility: retried across restarts rather than rediscovered by the gap sweep,
observable per token instead of per run, and drained at a rate that has nothing to do with how fast
blocks arrive. The gap-driven design already makes this a swap rather than a rewrite — the sweep
becomes the reconciler behind the queue rather than the mechanism.

**A more customisable RPC client than viem.** viem is pleasant, and three of its opinions cost real
work here: `fallback` deliberately hides which URL served a call (worked around with a per-transport
`fetchFn` so provider health is observable at all), the typed `getLogs` action silently ignores a
`topics` argument (worked around by calling `eth_getLogs` through `client.request`), and per-provider
retry is not configurable from the outside. A thinner client — or a JSON-RPC transport written
directly over `undici` — would make provider policy, batching and attribution first-class instead of
things to reach around. It is a real trade: viem's ABI encoding and error taxonomy are worth having,
and enrichment leans on both.

**Concurrency in sync and backfill.** Both are strictly linear today: one range, committed, then the
next. That is correct at the tip, where ordering is the point, and wasteful during a long backfill,
where ranges are independent and the only thing that must stay ordered is the cursor. Worth
_measuring_ rather than assuming — N ranges in flight with a windowed commit, bounded by the
provider's rate limit rather than by the loop's shape. The catch-up above is ~1,900 requests at ~1.4
ranges a second, and the bottleneck may well be the endpoint rather than the serialisation, which is
exactly why it wants a measurement before a rewrite. The ledger tolerates concurrency already: writes
are idempotent and the fold does not care what order rows arrive in.

Beyond those four, in rough priority:

- **Kubernetes manifests**, and a CI job that builds the images — the probes, drain and JSON logs are
  shaped for them, but nothing currently fails if the `Dockerfile` rots.
- **A single-writer guarantee.** A `pg_advisory_lock` on `chain_id` held for the process lifetime.
  Two indexers on one chain currently both write the same cursor row, which a rolling deploy creates
  for as long as the old pod drains.
- **A price history**, folded the way every other ledger here is, which is what makes `asOf` queries
  priceable and a past health factor reproducible.
- **Metrics for enrichment** — the last of the four operational questions without an answer. The
  number that matters is the gap between listed underlyings and stored rows.
- **A materialized `hub_assets_current`.** The read view collapses 29,614 rows to produce 17 on every
  page, and that table grows about 1.5 million rows a year.
