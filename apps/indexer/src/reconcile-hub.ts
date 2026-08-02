import 'reflect-metadata';

import { createClient } from '@clickhouse/client';
import { HUB_ABI } from '@aave-positions/events';
import { ClickHouseHubAssetStore, type HubAsset } from '@aave-positions/positions';
import { createPublicClient, http } from 'viem';
import { mainnet } from 'viem/chains';

import { clickHouseEnvSchema, envSchema } from './config/env';

export const USAGE = `Usage: reconcile:hub [--to <block>] [--assets <n>] [--absolute]

Compares the folded Hub asset state against the chain's own \`getAsset\`, at
zero tolerance (§9.2). Any nonzero drift is a bug, not noise.

Two modes, because what a node will serve decides which is possible.

  delta (default)  The fold is expected to hold exactly the events in
                   [from, to], and each additive field is checked against the
                   *difference* between getAsset at from-1 and at to. This is
                   §5.5's own experiment run against our fold, and it needs no
                   archive node — but the window is bounded by how far back
                   state is served, which on a public endpoint is ~127 blocks.
                   Truncate the Hub tables and backfill that range first.

  --absolute       Every field compared against getAsset outright. Requires the
                   fold to hold the asset's whole history, and therefore an
                   archive-capable RPC to have backfilled it.

Options:
  --from <block>   First block the fold was backfilled from. The baseline is
                   read at from-1, so this must match the backfill exactly or
                   the delta is measured against the wrong starting state.
  --to <block>     Last block of the comparison. Defaults to head - 20.
  --assets <n>     How many asset ids to check. Defaults to the Hub's own count.
  --absolute       Compare absolute values rather than a delta.
  -h, --help       This message.

Chain, providers and ClickHouse come from the environment, exactly as the
indexer reads them.
`;

/** The fields folded additively — checked as a difference in delta mode. */
const ADDITIVE = [
  'liquidity',
  'addedShares',
  'drawnShares',
  'swept',
  'premiumShares',
  'premiumOffsetRay',
  'deficitRay',
] as const;

/**
 * Latest-wins fields, which are absolute in both modes.
 *
 * A checkpoint is a value, not a delta — so as long as one `UpdateAsset` landed
 * inside the window, the fold should carry exactly what the chain has. If
 * none did, the fold holds null and the field is reported skipped rather than
 * counted as agreement.
 */
const LATEST_WINS = [
  'drawnIndex',
  'drawnRate',
  'realizedFees',
  'liquidityFee',
  // The checkpoint's own time, and the field the whole extrapolation hangs off:
  // `drawnIndex(t)` applies linear interest from *here*, so an hour's error
  // shifts every debt in the asset by an hour of interest. It is the one
  // latest-wins value the fold derives rather than reads — the event carries
  // no timestamp and `accrue()` sets `lastUpdateTimestamp = block.timestamp`,
  // so this checks that derivation against what the chain actually stored.
  'checkpointAt',
] as const;

/**
 * The chain's `getAsset` struct, reduced to the fields the fold keeps.
 *
 * Written out rather than indexed dynamically: this *is* the mapping between
 * the fold's column names and the contract's, and a typo here would compare
 * the wrong pair of numbers and report agreement.
 */
function chainFields(a: {
  liquidity: bigint;
  addedShares: bigint;
  drawnShares: bigint;
  swept: bigint;
  premiumShares: bigint;
  premiumOffsetRay: bigint;
  deficitRay: bigint;
  drawnIndex: bigint;
  drawnRate: bigint;
  realizedFees: bigint;
  liquidityFee: number;
  lastUpdateTimestamp: number;
}): Readonly<Record<string, bigint>> {
  return {
    liquidity: a.liquidity,
    addedShares: a.addedShares,
    drawnShares: a.drawnShares,
    swept: a.swept,
    premiumShares: a.premiumShares,
    premiumOffsetRay: a.premiumOffsetRay,
    deficitRay: a.deficitRay,
    drawnIndex: a.drawnIndex,
    drawnRate: a.drawnRate,
    realizedFees: a.realizedFees,
    liquidityFee: BigInt(a.liquidityFee),
    checkpointAt: BigInt(a.lastUpdateTimestamp),
  };
}

/**
 * The fold's value for a latest-wins field, as an integer.
 *
 * All of them are decimal strings or numbers except `index_timestamp`, which the
 * store formats as an ISO instant so a caller gets an unambiguous UTC time
 * rather than a locale-dependent one. Comparing it means going back to seconds.
 */
/**
 * How each latest-wins comparison reads its value off the fold row.
 *
 * Named one at a time rather than indexed dynamically: this is the mapping
 * between the comparison's field names and the row's, and a wrong pair would
 * compare two unrelated numbers and report agreement. Keyed by the field list,
 * so adding a comparison without an accessor does not compile.
 */
const FOLD_FIELD: Readonly<
  Record<(typeof LATEST_WINS)[number], (row: HubAsset) => string | number | null>
> = {
  drawnIndex: (row) => row.drawnIndex,
  drawnRate: (row) => row.drawnRate,
  realizedFees: (row) => row.realizedFees,
  liquidityFee: (row) => row.liquidityFee,
  // The store formats this as an ISO instant so a caller gets an unambiguous
  // UTC time; comparing it against the chain's uint40 means going back to
  // seconds.
  checkpointAt: (row) =>
    row.indexTimestamp === null ? null : Date.parse(row.indexTimestamp) / 1000,
};

function foldValue(row: HubAsset, field: (typeof LATEST_WINS)[number]): bigint | null {
  const value = FOLD_FIELD[field](row);
  return value === null ? null : BigInt(value);
}

interface Mismatch {
  readonly assetId: string;
  readonly field: string;
  readonly expected: string;
  readonly actual: string;
}

interface Args {
  from?: number;
  to?: number;
  assets?: number;
  absolute: boolean;
  help: boolean;
}

function parseArgs(argv: readonly string[]): Args {
  const out: Args = { absolute: false, help: false };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '-h' || arg === '--help') out.help = true;
    else if (arg === '--absolute') out.absolute = true;
    else if (arg === '--from') out.from = Number(argv[(i += 1)]);
    else if (arg === '--to') out.to = Number(argv[(i += 1)]);
    else if (arg === '--assets') out.assets = Number(argv[(i += 1)]);
  }
  return out;
}

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    process.stdout.write(USAGE);
    return;
  }

  const env = envSchema.parse(process.env);
  const ch = clickHouseEnvSchema.parse(process.env);

  const rpcUrl = env.RPC_URLS[0];
  const chain = createPublicClient({ chain: mainnet, transport: http(rpcUrl) });
  // Validated as a 20-byte hex address by the env schema; viem wants the
  // literal type, and there is no narrowing that proves it to the compiler.
  const hub: `0x${string}` = `0x${env.CORE_HUB_ADDRESS.slice(2)}`;

  const client = createClient({
    url: ch.CLICKHOUSE_URL,
    database: ch.CLICKHOUSE_DATABASE,
    username: ch.CLICKHOUSE_USER,
    password: ch.CLICKHOUSE_PASSWORD,
  });
  const store = new ClickHouseHubAssetStore(client);

  const head = Number(await chain.getBlockNumber());
  const to = args.to ?? head - 20;
  // The baseline is the block *before* the backfilled range, not before `to`:
  // an event at the start of the window is already in the fold, so measuring
  // from `to - 1` would compare it against a chain state that already contains
  // it and report the fold's own row as drift.
  const baseline = BigInt((args.from ?? to) - 1);

  const count =
    args.assets ??
    Number(await chain.readContract({ address: hub, abi: HUB_ABI, functionName: 'getAssetCount' }));

  const folded = await store.list(env.CHAIN_ID, hub);
  const byId = new Map(folded.map((a) => [a.assetId, a]));

  const getAsset = (assetId: number, blockNumber: bigint) =>
    chain.readContract({
      address: hub,
      abi: HUB_ABI,
      functionName: 'getAsset',
      args: [BigInt(assetId)],
      blockNumber,
    });

  const mismatches: Mismatch[] = [];
  let compared = 0;
  let skipped = 0;
  let untouched = 0;

  for (let assetId = 0; assetId < count; assetId += 1) {
    const id = String(assetId);
    const row: HubAsset | undefined = byId.get(id);
    if (!row) {
      // In delta mode an asset with no event in the window has no fold row,
      // which is the correct answer rather than a missing one. In absolute mode
      // every listed asset must be present.
      if (args.absolute) {
        mismatches.push({ assetId: id, field: '<row>', expected: 'present', actual: 'missing' });
      } else {
        untouched += 1;
      }
      continue;
    }

    // Sequential on purpose: a public endpoint rate-limits a burst of these
    // long before 17 calls become slow, and this runs once.
    // oxlint-disable-next-line no-await-in-loop
    const after = chainFields(await getAsset(assetId, BigInt(to)));
    // oxlint-disable-next-line no-await-in-loop
    const before = args.absolute ? null : chainFields(await getAsset(assetId, baseline));

    for (const field of ADDITIVE) {
      const expected = (after[field] ?? 0n) - (before?.[field] ?? 0n);
      const actual = BigInt(row[field]);
      compared += 1;
      if (expected !== actual) {
        mismatches.push({
          assetId: id,
          field,
          expected: expected.toString(),
          actual: actual.toString(),
        });
      }
    }

    for (const field of LATEST_WINS) {
      const value = foldValue(row, field);
      if (value === null) {
        // No event of that kind inside the window. Not agreement, and not
        // drift — say so rather than counting it either way.
        skipped += 1;
        continue;
      }
      const expected = after[field] ?? 0n;
      const actual = value;
      compared += 1;
      if (expected !== actual) {
        mismatches.push({
          assetId: id,
          field,
          expected: expected.toString(),
          actual: actual.toString(),
        });
      }
    }
  }

  const mode = args.absolute ? 'absolute' : `delta against block ${baseline}`;
  process.stdout.write(
    `hub ${hub}\n` +
      `${mode}, at block ${to}\n` +
      `${count} asset(s) listed, ${untouched} untouched in the window\n` +
      `${compared} field comparison(s), ${skipped} skipped (no such event in window)\n`,
  );

  if (mismatches.length === 0) {
    process.stdout.write(`\nzero drift.\n`);
  } else {
    process.stdout.write(`\n${mismatches.length} MISMATCH(ES):\n`);
    for (const m of mismatches) {
      process.stdout.write(
        `  asset ${m.assetId} ${m.field}: chain ${m.expected}, fold ${m.actual}\n`,
      );
    }
    process.exitCode = 1;
  }

  await client.close();
}

void main().catch((error: unknown) => {
  process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`);
  process.exitCode = 1;
});
