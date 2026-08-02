import type { ClickHouseClient } from '@clickhouse/client';
import {
  ClickHouseHubEventStore,
  ClickHouseSpokeEventStore,
  type DecodedEvent,
} from '@aave-positions/events';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import { CHAIN_ID, SPOKE, TABLES, migratedDatabase } from '../test-support/spoke-ledger';

/** Its own database: sibling suites share table names and would truncate this one. */
const DATABASE = 'spec_ledger_separation';
const HUB = '0xcca852bc40e560adc3b1cc58ca5b55638ce826c9';

let client: ClickHouseClient;
let spoke: ClickHouseSpokeEventStore;
let hub: ClickHouseHubEventStore;

/**
 * The Hub's `ReportDeficit`, which shares a *name* with the Spoke's and nothing
 * else — five parameters against four, keyed by `(assetId, spoke)` rather than
 * `(reserveId, user)`, and carrying no `user` at all (§4.4).
 */
const hubReportDeficit = (address: string): DecodedEvent => ({
  chainId: CHAIN_ID,
  address,
  blockNumber: 1000,
  blockHash: `0x${'aa'.repeat(32)}`,
  blockTimestamp: 1_785_000_000,
  txHash: `0x${'bb'.repeat(32)}`,
  txIndex: 0,
  logIndex: 0,
  eventName: 'ReportDeficit',
  topic1: `0x${'07'.padStart(64, '0')}`,
  topic2: `0x${SPOKE.slice(2).padStart(64, '0')}`,
  topic3: null,
  body: {
    assetId: '7',
    spoke: SPOKE,
    drawnShares: '900',
    premiumDelta: { sharesDelta: '0', offsetRayDelta: '0', restoredPremiumRay: '0' },
    deficitAmountRay: '2950000000000000000000000000000000000000000000',
  },
  data: '0x00',
});

const count = async (table: string): Promise<number> => {
  const result = await client.query({
    query: `SELECT count() AS n FROM ${table}`,
    format: 'JSONEachRow',
  });
  return Number((await result.json<{ n: string | number }>())[0]?.n);
};

describe('two ledgers, because one would collide', () => {
  beforeAll(async () => {
    client = await migratedDatabase(DATABASE);
    spoke = new ClickHouseSpokeEventStore(client);
    hub = new ClickHouseHubEventStore(client);
  });

  afterAll(async () => {
    await client.close();
  });

  beforeEach(async () => {
    for (const table of [...TABLES, 'hub_events'])
      // oxlint-disable-next-line no-await-in-loop
      await client.command({ query: `TRUNCATE TABLE ${table}` });
  });

  it('rejects a Hub event written into the Spoke ledger', async () => {
    // The position projection filters on `event_name` alone — it has no address
    // predicate and cannot get one, since the Spoke address is configuration
    // rather than something a migration knows. So a Hub `ReportDeficit` in
    // `spoke_events` fires it, it reaches for the absent `reserveId`, and
    // `toUInt256('')` throws rather than writing a zero.
    await expect(spoke.append([hubReportDeficit(HUB)])).rejects.toThrow(/Cannot parse UInt256/);
  });

  it('leaves the ledger row committed when the projection throws', async () => {
    await expect(spoke.append([hubReportDeficit(HUB)])).rejects.toThrow(/Cannot parse UInt256/);

    // This is the part that makes the collision unrecoverable rather than
    // merely loud, and it is measured rather than assumed: the insert reports
    // failure, the ledger row lands anyway, and the projection row does not.
    // Ingestion then jams on a range that will never succeed, and if the view
    // is later fixed the next dispatch retracts a row the projection never
    // received — leaving the position short by exactly that event.
    expect(await count('spoke_events')).toBe(1);
    expect(await count('user_positions')).toBe(0);
  });

  it('accepts the same event in its own ledger, with no projection disturbed', async () => {
    await hub.append([hubReportDeficit(HUB)]);

    // Same bytes, same name, different table — and nothing fires. That is the
    // whole argument for the separation.
    expect(await count('hub_events_current')).toBe(1);
    expect(await count('user_positions')).toBe(0);
  });

  it('still folds the Spoke ReportDeficit it was always meant to', async () => {
    await spoke.append([
      {
        ...hubReportDeficit(SPOKE),
        body: {
          reserveId: '7',
          user: SPOKE,
          drawnShares: '900',
          premiumDelta: { sharesDelta: '0', offsetRayDelta: '0', restoredPremiumRay: '0' },
        },
      },
    ]);

    // The four-parameter form, which is a real Spoke event and must keep
    // working. Guards against "fixing" the collision by narrowing the view
    // until it matches nothing.
    expect(await count('user_positions')).toBe(1);
  });
});
