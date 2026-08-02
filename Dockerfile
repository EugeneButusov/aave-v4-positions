# syntax=docker/dockerfile:1

# One Dockerfile for both services; compose passes APP=api or APP=indexer.
ARG NODE_VERSION=24

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
COPY packages/migrations/package.json ./packages/migrations/
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
