# syntax=docker/dockerfile:1

# Three targets. `migrate` is the Rust binary that applies the schema; `runtime`
# is either Node service, chosen with APP=api or APP=indexer.
#
# **Every consumer names its target.** `runtime` is last so a bare `docker build`
# still produces a service image, but compose says which one it wants either way
# — appending a stage must not be able to change what an existing service is.
ARG NODE_VERSION=24
ARG RUST_VERSION=1.96
ARG ALPINE_VERSION=3.24

# ------------------------------------------------------------- rust-build ----
# The schema is applied by a Rust binary, so this stage shares no layer with the
# Node ones above. `rust:*-alpine` builds against musl, which is what the runtime
# below wants anyway.
FROM rust:${RUST_VERSION}-alpine${ALPINE_VERSION} AS rust-build
# The linker and crt objects. Nothing more: the dependency tree is pure Rust —
# no `-sys` crates and no build script that shells out to a C compiler — which
# is what makes a musl build this uneventful.
RUN apk add --no-cache musl-dev
WORKDIR /src

# rust-toolchain.toml first, so rustup installs the pinned toolchain rather than
# whatever the image tag happens to carry.
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY bins ./bins

# The SQL, named one directory at a time rather than copying `packages`.
# `bins/migrate/src/schema.rs` embeds every file with `include_str!` and the
# paths reach back here, so these are a build input; listing them exactly keeps
# a TypeScript edit from invalidating this layer. They come home in Phase 5.
COPY packages/aave-positions/events/src/store/clickhouse-migrations ./packages/aave-positions/events/src/store/clickhouse-migrations
COPY packages/aave-positions/positions/src/store/clickhouse-migrations ./packages/aave-positions/positions/src/store/clickhouse-migrations
COPY packages/indexing/src/postgres-migrations ./packages/indexing/src/postgres-migrations
COPY packages/token-metadata/src/migrations ./packages/token-metadata/src/migrations
COPY packages/prices/src/migrations ./packages/prices/src/migrations

RUN cargo build --release --locked -p migrate

# ---------------------------------------------------------------- migrate ----
FROM alpine:${ALPINE_VERSION} AS migrate
# The image is the binary. It reads no file at runtime — every `.sql` is
# compiled in and the whole configuration is five environment variables — so
# there is nothing to copy beside it and nothing to mount.
#
# Alpine rather than scratch: the binary is musl-linked either way, and one
# shell is worth having the first time a migration fails inside a container.
COPY --from=rust-build /src/target/release/migrate /usr/local/bin/migrate
USER nobody

# Exec form and no shell, so a `docker compose down` mid-migration reaches the
# process. There is nothing to drain — a failed run leaves the ledger accurate.
ENTRYPOINT ["/usr/local/bin/migrate"]

# ---------------------------------------------------------------- build ------
FROM node:${NODE_VERSION}-alpine AS build
ARG APP
RUN corepack enable
WORKDIR /repo

# Manifests first: this layer is cached until a dependency actually changes,
# so editing source does not re-resolve the whole workspace.
COPY pnpm-workspace.yaml pnpm-lock.yaml package.json .npmrc ./
COPY apps/api/package.json ./apps/api/
COPY apps/indexer/package.json ./apps/indexer/
COPY packages/aave-positions/events/package.json ./packages/aave-positions/events/
COPY packages/aave-positions/positions/package.json ./packages/aave-positions/positions/
COPY packages/clickhouse/package.json ./packages/clickhouse/
COPY packages/indexing/package.json ./packages/indexing/
COPY packages/ops/package.json ./packages/ops/
COPY packages/postgres/package.json ./packages/postgres/
COPY packages/prices/package.json ./packages/prices/
COPY packages/telemetry/package.json ./packages/telemetry/
COPY packages/token-metadata/package.json ./packages/token-metadata/

# --ignore-scripts skips the `prepare` hook, which installs git hooks: there is
# no git repository here, and the image needs none. Everything prepare would
# have done is run explicitly below.
RUN pnpm install --frozen-lockfile --ignore-scripts

COPY tsconfig.base.json tsconfig.json ./
COPY packages ./packages
COPY apps ./apps

RUN pnpm run build:packages && pnpm --filter="@aave-v4-positions/${APP}" build

# Collects the app, its built workspace dependencies and production-only
# node_modules into one self-contained directory.
RUN pnpm --filter="@aave-v4-positions/${APP}" --legacy deploy --prod --ignore-scripts /out

# -------------------------------------------------------------- runtime ------
FROM node:${NODE_VERSION}-alpine AS runtime
ENV NODE_ENV=production
WORKDIR /app

# The node images ship an unprivileged `node` user; nothing here needs root.
COPY --from=build --chown=node:node /out ./
USER node

# Exec form, so the process is PID 1 and receives SIGTERM directly — a shell
# wrapper would swallow it and the graceful drain would never run.
#
# `--require` and not `NODE_OPTIONS`. NODE_OPTIONS would be tidier — one line,
# covering `docker compose run indexer node dist/backfill.js` as well — but the
# compose healthchecks run `node -e "fetch('…/health/ready')"` every ten
# seconds, and under NODE_OPTIONS each of those would boot a full SDK, open
# exporters and register instrumentations six times a minute per container, for
# a process that lives about fifty milliseconds. That is a trap laid for
# whoever next edits the healthcheck. Per-entrypoint `--require` cannot do it.
#
# Resolved through `/app/node_modules`, which is where `pnpm deploy` puts the
# workspace packages. `OTEL_SDK_DISABLED=true` makes it a no-op without a
# rebuild, which is what the overhead A/B in the README relies on.
CMD ["node", "--require", "@packages/telemetry/start", "--enable-source-maps", "dist/main.js"]
