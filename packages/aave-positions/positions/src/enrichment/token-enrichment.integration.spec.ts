import type { ClickHouseClient } from '@clickhouse/client';
import { ClickHouseHubEventStore } from '@aave-positions/events';
import { loadMigrations } from '@packages/migrations';
import { migrate } from '@packages/postgres';
import { ViemErc20MetadataReader } from '@packages/indexing';
import postgres from 'postgres';
import { encodeAbiParameters, type Hex } from 'viem';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';

import { POSITION_POSTGRES_MIGRATIONS_DIR } from '../postgres-migrations';
import { PostgresTokenMetadataStore } from '../store/postgres-token-metadata-store';
import { ClickHouseTokenListings } from '../store/token-listing-source';
import { HUB_TABLES, addAsset, updateAsset } from '../test-support/hub-ledger';
import { CHAIN_ID, migratedDatabase } from '../test-support/spoke-ledger';
import { TokenEnrichmentProcessor } from './token-enrichment.processor';

/**
 * The wiring, end to end, against both real databases.
 *
 * The unit specs pin each piece against fakes; what none of them covers is the
 * pieces being connected to each other — a listing query reading the wrong
 * column, a token address that arrives checksummed on one side and lower-cased
 * on the other, a row that maps cleanly in isolation and lands crossed.
 *
 * Only the node is stubbed, and only because pointing this at mainnet would
 * make it a network test. The mainnet run — all seventeen underlyings, with
 * `decimals` cross-checked against the address book — is `enrich:tokens`
 * against a real RPC, which is a step an operator takes.
 */
const DATABASE = 'spec_enrichment_e2e';
const SCHEMA = 'enrichment_e2e_spec';
const PG_URL = process.env['POSTGRES_URL'] ?? 'postgres://aave:aave@localhost:5432/aave';

const HEAD = 25_652_782;
const USDC = '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48';
const WETH = '0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2';
const MUTE = '0x00000000000000000000000000000000000000ff';

const SELECTOR = { symbol: '0x95d89b41', name: '0x06fdde03', decimals: '0x313ce567' } as const;

const NEVER_ABORTED = new AbortController().signal;

const asString = (value: string): Hex => encodeAbiParameters([{ type: 'string' }], [value]);
const asUint8 = (value: number): Hex => encodeAbiParameters([{ type: 'uint8' }], [value]);

/** What each token answers, by address then by selector. */
const CHAIN: Record<string, Partial<Record<keyof typeof SELECTOR, Hex>>> = {
  [USDC]: { symbol: asString('USDC'), name: asString('USD Coin'), decimals: asUint8(6) },
  [WETH]: { symbol: asString('WETH'), name: asString('Wrapped Ether'), decimals: asUint8(18) },
  // Implements none of the three, which ERC-20 permits.
  [MUTE]: {},
};

function reply(result: unknown): Response {
  return new Response(JSON.stringify({ jsonrpc: '2.0', id: 1, result }), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
}

function stubNode(): void {
  vi.stubGlobal(
    'fetch',
    vi.fn((_input: string | URL | Request, init?: RequestInit) => {
      const body: unknown = JSON.parse(typeof init?.body === 'string' ? init.body : '{}');
      const { method, params } = body as {
        method?: string;
        params?: [{ to?: string; data?: Hex }];
      };

      if (method === 'eth_blockNumber') return Promise.resolve(reply(`0x${HEAD.toString(16)}`));

      const to = params?.[0]?.to?.toLowerCase() ?? '';
      const calldata = params?.[0]?.data ?? '0x';
      const entry = Object.entries(SELECTOR).find(([, sel]) => calldata.startsWith(sel));
      const answer = entry ? CHAIN[to]?.[entry[0] as keyof typeof SELECTOR] : undefined;

      return Promise.resolve(reply(answer ?? '0x'));
    }),
  );
}

let client: ClickHouseClient;
let events: ClickHouseHubEventStore;
let processor: TokenEnrichmentProcessor;
const sql = postgres(PG_URL, { max: 2, connection: { search_path: SCHEMA } });

describe('token enrichment, end to end', () => {
  beforeAll(async () => {
    const admin = postgres(PG_URL, { max: 1 });
    try {
      await admin`CREATE SCHEMA IF NOT EXISTS ${admin(SCHEMA)}`;
    } finally {
      await admin.end();
    }
    await migrate(sql, await loadMigrations([POSITION_POSTGRES_MIGRATIONS_DIR]));

    client = await migratedDatabase(DATABASE);
    events = new ClickHouseHubEventStore(client);
  });

  afterAll(async () => {
    await client.close();
    await sql.end();
  });

  beforeEach(async () => {
    stubNode();
    await sql`TRUNCATE token_metadata`;
    for (const table of HUB_TABLES)
      // oxlint-disable-next-line no-await-in-loop
      await client.command({ query: `TRUNCATE TABLE ${table}` });

    processor = new TokenEnrichmentProcessor(
      { chainId: CHAIN_ID, sweepIntervalMs: 300_000, concurrency: 4 },
      new ClickHouseTokenListings(client),
      new PostgresTokenMetadataStore(sql),
      new ViemErc20MetadataReader({ rpcUrls: ['https://node.invalid'], rpcTimeoutMs: 1_000 }),
      {
        getChainId: () => Promise.resolve(CHAIN_ID),
        getHeadBlockNumber: () => Promise.resolve(HEAD),
        getBlockHeader: () => {
          throw new Error('unused');
        },
      },
    );
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('takes a cold database from AddAsset to a labelled row', async () => {
    // Genesis, far behind any cursor — so this is the sweep's path, and the
    // fast path would find nothing.
    await events.append([
      addAsset({ block: 24_722_784, log: 0 }, USDC, 6, '1'),
      addAsset({ block: 24_722_784, log: 1 }, WETH, 18, '2'),
    ]);

    await processor.onBlockRange(25_000_000, 25_001_000, NEVER_ABORTED);

    const rows = await sql`SELECT token, symbol, name, token_decimals, fetched_at_block
                           FROM token_metadata ORDER BY token`;
    expect(rows).toHaveLength(2);
    expect(rows[0]).toMatchObject({
      token: USDC,
      symbol: 'USDC',
      name: 'USD Coin',
      token_decimals: 6,
      fetched_at_block: String(HEAD),
    });
    expect(rows[1]).toMatchObject({ token: WETH, symbol: 'WETH', token_decimals: 18 });
  });

  it('carries a token whose decimals disagree with the Hub', async () => {
    // The Hub listed 18; the token says 6. Both are stored, and nothing about
    // the position's arithmetic changes — the Hub's value is what values it.
    // This is a listing audit signal, and the only place it becomes visible.
    await events.append([addAsset({ block: 24_722_784 }, USDC, 18, '1')]);

    await processor.onBlockRange(25_000_000, 25_001_000, NEVER_ABORTED);

    const [row] = await sql`SELECT token_decimals FROM token_metadata WHERE token = ${USDC}`;
    const hub = await client.query({
      query: `SELECT decimals FROM hub_assets_current WHERE chain_id = ${CHAIN_ID}`,
      format: 'JSONEachRow',
    });
    expect(row?.['token_decimals']).toBe(6);
    expect((await hub.json<{ decimals: number }>())[0]?.decimals).toBe(18);
  });

  it('records a mute token so it is never re-read', async () => {
    await events.append([addAsset({ block: 24_722_784 }, MUTE, 6, '1')]);

    await processor.onBlockRange(25_000_000, 25_001_000, NEVER_ABORTED);
    const [stored] = await sql`SELECT symbol, name FROM token_metadata WHERE token = ${MUTE}`;
    expect(stored).toMatchObject({ symbol: null, name: null });

    // Second sweep: the row is what tells it not to ask again.
    vi.useFakeTimers();
    vi.advanceTimersByTime(300_000);
    vi.useRealTimers();
    await processor.onBlockRange(25_001_000, 25_002_000, NEVER_ABORTED);

    const count = await sql`SELECT count(*)::int AS c FROM token_metadata`;
    expect(count[0]?.['c']).toBe(1);
  });

  it('picks up a new listing through the fast path, in its own range', async () => {
    await events.append([addAsset({ block: 24_722_784 }, USDC, 6, '1')]);
    await processor.onBlockRange(25_000_000, 25_001_000, NEVER_ABORTED);

    // A governance listing, long after genesis and inside the sweep interval.
    await events.append([
      addAsset({ block: 25_500_000 }, WETH, 18, '2'),
      updateAsset({ block: 25_500_001 }, undefined, undefined, undefined, '2'),
    ]);
    await processor.onBlockRange(25_499_000, 25_501_000, NEVER_ABORTED);

    const [row] = await sql`SELECT symbol FROM token_metadata WHERE token = ${WETH}`;
    expect(row?.['symbol']).toBe('WETH');
  });
});
