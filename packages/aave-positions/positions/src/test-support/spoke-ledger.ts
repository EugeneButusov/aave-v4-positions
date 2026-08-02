import { createClient, type ClickHouseClient } from '@clickhouse/client';
import { EVENT_MIGRATIONS_DIR, type DecodedEvent } from '@aave-positions/events';
import { migrate } from '@packages/clickhouse';
import { loadMigrations } from '@packages/migrations';
import type { Address } from '@packages/indexing';

import { POSITION_MIGRATIONS_DIR } from '../store/clickhouse-position-store';

export const CHAIN_ID = 1;
export const SPOKE: Address = '0x94e7a5dcbe816e498b89ab752661904e2f56c485';
/**
 * The Bluechip Spoke — a second isolated margin account for the same wallet.
 * Sorts *after* {@link SPOKE} as a string (`0x94…` < `0x97…`), which is what
 * makes a cross-Spoke page order assertable rather than incidental.
 */
export const SECOND_SPOKE: Address = '0x973a023a77420ba610f06b3858ad991df6d85a08';
/** Checksummed, as viem hands them back. Lower-casing them is the fold's job. */
export const ALICE = '0x82D16fF1C724ab72F218A3f7f6DD3E5385ee87E8';
export const BOB = '0xB8516f75DCf450b5b455b5114F5a92F6abD37dCa';
/** A position manager, and never the owner of a position (§2). */
export const ROUTER = '0xe68ab4F90Fe026B9873F5F276eD2d7efBbbE42Be';
/** Past 2^53 and past Int64 max, which is where JSONExtractInt returns 0. */
export const HUGE = '422166581625087607993';

export interface At {
  readonly block: number;
  readonly log?: number;
  /** Which Spoke emitted it. Defaults to {@link SPOKE}. */
  readonly spoke?: Address;
}

/** A decoded log, shaped as the decoder would hand it over. */
export function event(name: string, at: At, body: Record<string, unknown>): DecodedEvent {
  return {
    chainId: CHAIN_ID,
    address: at.spoke ?? SPOKE,
    blockNumber: at.block,
    blockHash: `0x${'aa'.repeat(32)}`,
    blockTimestamp: 1_785_000_000,
    txHash: `0x${'bb'.repeat(32)}`,
    txIndex: 0,
    logIndex: at.log ?? 0,
    eventName: name,
    topic1: null,
    topic2: null,
    topic3: null,
    body,
    data: '0x',
  };
}

export const NO_PREMIUM = { sharesDelta: '0', offsetRayDelta: '0', restoredPremiumRay: '0' };

export const supply = (at: At, user: string, reserveId: string, shares: string, caller = user) =>
  event('Supply', at, { reserveId, caller, user, suppliedShares: shares, suppliedAmount: shares });

export const withdraw = (at: At, user: string, reserveId: string, shares: string) =>
  event('Withdraw', at, {
    reserveId,
    caller: user,
    user,
    withdrawnShares: shares,
    withdrawnAmount: shares,
  });

export const borrow = (at: At, user: string, reserveId: string, shares: string) =>
  event('Borrow', at, { reserveId, caller: user, user, drawnShares: shares, drawnAmount: shares });

export const repay = (
  at: At,
  user: string,
  reserveId: string,
  shares: string,
  premium = NO_PREMIUM,
) =>
  event('Repay', at, {
    reserveId,
    caller: user,
    user,
    drawnShares: shares,
    totalAmountRepaid: shares,
    premiumDelta: premium,
  });

export const setCollateral = (at: At, user: string, reserveId: string, on: boolean) =>
  event('SetUsingAsCollateral', at, { reserveId, caller: user, user, usingAsCollateral: on });

export const reportDeficit = (at: At, user: string, reserveId: string, shares: string) =>
  event('ReportDeficit', at, { reserveId, user, drawnShares: shares, premiumDelta: NO_PREMIUM });

export const addReserve = (at: At, reserveId: string, assetId: string, hub: string) =>
  event('AddReserve', at, { reserveId, assetId, hub });

/**
 * The config events, shaped as the decoder hands them over.
 *
 * **The addresses are checksummed on purpose.** viem decodes an indexed address
 * that way, so a projection that forgets to `lower()` writes a second,
 * unjoinable row for the same wallet — and it reads as a user who simply has no
 * config rather than as a bug. A fixture that pre-lowered them would never
 * catch it.
 */
export const addDynamicConfig = (
  at: At,
  reserveId: string,
  dynamicConfigKey: number,
  collateralFactor: number,
) =>
  event('AddDynamicReserveConfig', at, {
    reserveId,
    dynamicConfigKey,
    config: { collateralFactor, maxLiquidationBonus: 1_055_500, liquidationFee: 1_000 },
  });

export const updateDynamicConfig = (
  at: At,
  reserveId: string,
  dynamicConfigKey: number,
  collateralFactor: number,
) =>
  event('UpdateDynamicReserveConfig', at, {
    reserveId,
    dynamicConfigKey,
    config: { collateralFactor, maxLiquidationBonus: 1_055_500, liquidationFee: 1_000 },
  });

/** Carries no version: the user moves to whatever the reserve's key is here. */
export const refreshAllUserConfig = (at: At, user: string) =>
  event('RefreshAllUserDynamicConfig', at, { user });

export const refreshUserConfig = (at: At, user: string, reserveId: string) =>
  event('RefreshSingleUserDynamicConfig', at, { user, reserveId });

export const updateReserveConfig = (
  at: At,
  reserveId: string,
  flags: Partial<{ paused: boolean; frozen: boolean; borrowable: boolean }> = {},
) =>
  event('UpdateReserveConfig', at, {
    reserveId,
    config: {
      collateralRisk: 0,
      paused: false,
      frozen: false,
      borrowable: true,
      receiveSharesEnabled: false,
      ...flags,
    },
  });

export function liquidate(
  at: At,
  parts: {
    user: string;
    liquidator: string;
    collateralReserveId: string;
    debtReserveId: string;
    collateralSharesLiquidated: string;
    drawnSharesLiquidated: string;
    receiveShares?: boolean;
    collateralSharesToLiquidator?: string;
  },
): DecodedEvent {
  return event('LiquidationCall', at, {
    collateralReserveId: parts.collateralReserveId,
    debtReserveId: parts.debtReserveId,
    user: parts.user,
    liquidator: parts.liquidator,
    receiveShares: parts.receiveShares ?? false,
    debtAmountRestored: parts.drawnSharesLiquidated,
    drawnSharesLiquidated: parts.drawnSharesLiquidated,
    premiumDelta: NO_PREMIUM,
    collateralAmountRemoved: parts.collateralSharesLiquidated,
    collateralSharesLiquidated: parts.collateralSharesLiquidated,
    collateralSharesToLiquidator: parts.collateralSharesToLiquidator ?? '0',
  });
}

/** Tables a suite truncates between tests. Nothing in the packages removes a row. */
export const TABLES = ['spoke_events', 'user_positions', 'user_position_flags', 'spoke_reserves'];

const CONNECTION = {
  url: process.env['CLICKHOUSE_URL'] ?? 'http://localhost:8123',
  username: process.env['CLICKHOUSE_USER'] ?? 'default',
  password: process.env['CLICKHOUSE_PASSWORD'] ?? '',
};

/**
 * A migrated database of the caller's own.
 *
 * **One per suite, and not a tidiness preference.** Every suite here shares the
 * same server and the same table names, so a shared database has them truncating
 * each other's fixtures mid-run — and the events package's suite would find the
 * projections created here firing on its synthetic bodies, which are minimal by
 * design and carry no `reserveId`. Its specs would fail for a reason that has
 * nothing to do with what they test.
 *
 * Both migration directories, ordered by ordinal across them, which is what puts
 * the projections after the table they read.
 */
export async function migratedDatabase(name: string): Promise<ClickHouseClient> {
  const bootstrap = createClient(CONNECTION);
  // **Dropped first, not `IF NOT EXISTS`.** `migrate` skips ids already in
  // `schema_migrations`, so a database left behind by an earlier run keeps
  // whatever schema it was first created with — editing a migration would then
  // have no effect locally, and the suite would go on testing a table that no
  // longer matches the file. Found the hard way: a mutation that removed a
  // projection from `021` left every spec green here and would have failed only
  // in CI, where the server is always fresh.
  await bootstrap.command({ query: `DROP DATABASE IF EXISTS ${name}` });
  await bootstrap.command({ query: `CREATE DATABASE ${name}` });
  await bootstrap.close();

  const client = createClient({ ...CONNECTION, database: name });
  await migrate(client, await loadMigrations([EVENT_MIGRATIONS_DIR, POSITION_MIGRATIONS_DIR]));
  return client;
}
