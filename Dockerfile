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
CMD ["node", "--enable-source-maps", "dist/main.js"]
