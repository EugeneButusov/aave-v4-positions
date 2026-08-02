import 'reflect-metadata';

import { createClient } from '@clickhouse/client';
import { SPOKE_ABI } from '@aave-positions/events';
import { ClickHousePositionStore } from '@aave-positions/positions';
import { createPublicClient, http } from 'viem';
import { mainnet } from 'viem/chains';

import { clickHouseEnvSchema, envSchema } from './config/env';

export const USAGE = `Usage: reconcile:positions --users <a,b,c> [--at <block>]

Compares every valued position the store serves against the Spoke's own
\`getUserSuppliedAssets\` and \`getUserDebt\`, at zero tolerance (§9.2).

This is the composition check. The arithmetic on its own is validated against
the same getters by feeding them the chain's \`getAsset\` directly; the fold on
its own is validated by \`reconcile:hub\`. What only this can catch is the two
being wired together wrongly — a reserve resolved to the wrong asset, a
checkpoint read from the wrong column, a timestamp off by a block.

**Both sides are pinned to one block.** The chain is read at \`--at\`, and the
store is valued at that block's timestamp — the index accrues every second, so
comparing against \`latest\` would drift by however long the calls took.

Requires the fold to hold full history, and therefore an archive-capable RPC
to have backfilled it: the reserve registry comes from \`AddReserve\`, which fires
once per reserve at the Spoke's genesis.

Options:
  --users <list>   Comma-separated addresses. Required.
  --at <block>     Block to pin both sides to. Defaults to head - 20.
  -h, --help       This message.
`;

interface Mismatch {
  readonly user: string;
  readonly reserveId: string;
  readonly field: string;
  readonly chain: bigint;
  readonly ours: bigint;
}

async function main(): Promise<void> {
  const argv = process.argv.slice(2);
  if (argv.includes('-h') || argv.includes('--help')) {
    process.stdout.write(USAGE);
    return;
  }

  const users = (argv[argv.indexOf('--users') + 1] ?? '')
    .split(',')
    .map((u) => u.trim())
    .filter((u) => u.length > 0);
  if (argv.indexOf('--users') === -1 || users.length === 0) {
    process.stderr.write(`--users is required\n\n${USAGE}`);
    process.exitCode = 2;
    return;
  }
  const atArg = argv.indexOf('--at') === -1 ? undefined : Number(argv[argv.indexOf('--at') + 1]);

  const env = envSchema.parse(process.env);
  const ch = clickHouseEnvSchema.parse(process.env);

  const chain = createPublicClient({ chain: mainnet, transport: http(env.RPC_URLS[0]) });
  const spoke: `0x${string}` = `0x${env.MAIN_SPOKE_ADDRESS.slice(2)}`;

  const client = createClient({
    url: ch.CLICKHOUSE_URL,
    database: ch.CLICKHOUSE_DATABASE,
    username: ch.CLICKHOUSE_USER,
    password: ch.CLICKHOUSE_PASSWORD,
  });
  const store = new ClickHousePositionStore(client);

  const head = Number(await chain.getBlockNumber());
  const blockNumber = BigInt(atArg ?? head - 20);
  const block = await chain.getBlock({ blockNumber });

  const mismatches: Mismatch[] = [];
  let compared = 0;
  let unvalued = 0;
  let positions = 0;

  for (const user of users) {
    // oxlint-disable-next-line no-await-in-loop
    const page = await store.list({
      chainId: env.CHAIN_ID,
      user,
      spoke: env.MAIN_SPOKE_ADDRESS,
      limit: 500,
      asOf: block.timestamp,
    });

    for (const position of page.items) {
      positions += 1;
      if (position.value === null) {
        unvalued += 1;
        continue;
      }

      const account: `0x${string}` = `0x${user.replace(/^0x/, '')}`;
      const args = [BigInt(position.reserveId), account] as const;
      // oxlint-disable-next-line no-await-in-loop
      const supplied: bigint = await chain.readContract({
        address: spoke,
        abi: SPOKE_ABI,
        functionName: 'getUserSuppliedAssets',
        args,
        blockNumber,
      });
      // oxlint-disable-next-line no-await-in-loop
      const debt: readonly [bigint, bigint] = await chain.readContract({
        address: spoke,
        abi: SPOKE_ABI,
        functionName: 'getUserDebt',
        args,
        blockNumber,
      });
      const [drawn, premium] = debt;

      const cases: readonly [string, bigint, bigint][] = [
        ['suppliedAmount', supplied, BigInt(position.value.suppliedAmount)],
        ['drawnDebt', drawn, BigInt(position.value.drawnDebt)],
        ['premiumDebt', premium, BigInt(position.value.premiumDebt)],
      ];
      for (const [field, chainValue, ours] of cases) {
        compared += 1;
        if (chainValue !== ours) {
          mismatches.push({ user, reserveId: position.reserveId, field, chain: chainValue, ours });
        }
      }
    }
  }

  process.stdout.write(
    `spoke ${env.MAIN_SPOKE_ADDRESS}\n` +
      `block ${blockNumber}, valued at t=${block.timestamp}\n` +
      `${users.length} wallet(s), ${positions} position(s), ${unvalued} unvalued (no registry or checkpoint)\n` +
      `${compared} field comparison(s)\n`,
  );

  if (mismatches.length === 0) {
    process.stdout.write(`\nzero drift.\n`);
  } else {
    process.stdout.write(`\n${mismatches.length} MISMATCH(ES):\n`);
    for (const m of mismatches) {
      process.stdout.write(
        `  ${m.user} reserve ${m.reserveId} ${m.field}: chain ${m.chain}, ours ${m.ours} (${m.ours - m.chain > 0n ? '+' : ''}${m.ours - m.chain})\n`,
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
