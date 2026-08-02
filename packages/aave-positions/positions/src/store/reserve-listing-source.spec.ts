import type { ClickHouseClient } from '@clickhouse/client';
import { ClickHouseSpokeEventStore, type DecodedEvent } from '@aave-positions/events';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import {
  CHAIN_ID,
  SECOND_SPOKE,
  SPOKE,
  TABLES,
  addReserve,
  migratedDatabase,
} from '../test-support/spoke-ledger';
import { ClickHouseReserveListings } from './reserve-listing-source';

/** Its own database: sibling suites share table names and would truncate this one. */
const DATABASE = 'spec_reserve_listings';

const HUB = '0xcca852bc40e560adc3b1cc58ca5b55638ce826c9';

describe('ClickHouseReserveListings', () => {
  let client: ClickHouseClient;
  let events: ClickHouseSpokeEventStore;
  let listings: ClickHouseReserveListings;

  /** What the processor does with a dispatched range: cancel, then write. */
  async function index(from: number, to: number, batch: DecodedEvent[]): Promise<void> {
    await events.revert(CHAIN_ID, from, to);
    await events.append(batch);
  }

  beforeAll(async () => {
    client = await migratedDatabase(DATABASE);
    events = new ClickHouseSpokeEventStore(client);
    listings = new ClickHouseReserveListings(client);
  });

  afterAll(async () => {
    await client.close();
  });

  beforeEach(async () => {
    for (const table of TABLES) {
      // oxlint-disable-next-line no-await-in-loop
      await client.command({ query: `TRUNCATE TABLE ${table}` });
    }
  });

  it('lists what the Spoke registered', async () => {
    await index(100, 100, [
      addReserve({ block: 100, log: 0 }, '0', '1', HUB),
      addReserve({ block: 100, log: 1 }, '1', '2', HUB),
    ]);

    expect(await listings.forSpoke(CHAIN_ID, SPOKE)).toEqual(['0', '1']);
  });

  it('orders by number rather than by digits', async () => {
    // The trap `clickhouse-position-store.ts` documents from the other side:
    // unqualified, `reserve_id` binds to the `toString` alias and sorts as
    // text, putting 13 before 3. There it handed the next page a key from the
    // wrong row; here it would hand the oracle a differently ordered batch than
    // the one this spec pins.
    await index(100, 100, [
      addReserve({ block: 100, log: 0 }, '13', '1', HUB),
      addReserve({ block: 100, log: 1 }, '3', '2', HUB),
      addReserve({ block: 100, log: 2 }, '0', '3', HUB),
    ]);

    expect(await listings.forSpoke(CHAIN_ID, SPOKE)).toEqual(['0', '3', '13']);
  });

  it('keeps two spokes apart', async () => {
    // Each Spoke has its own oracle over its own id space (§12.3), so reserve 0
    // on one has nothing to do with reserve 0 on the other. Mixing them would
    // ask one oracle about an id it has never heard of.
    await index(100, 100, [
      addReserve({ block: 100, log: 0 }, '0', '1', HUB),
      addReserve({ block: 100, log: 1, spoke: SECOND_SPOKE }, '7', '2', HUB),
    ]);

    expect(await listings.forSpoke(CHAIN_ID, SPOKE)).toEqual(['0']);
    expect(await listings.forSpoke(CHAIN_ID, SECOND_SPOKE)).toEqual(['7']);
  });

  it('drops a registration a reorg took back', async () => {
    // **The deliberate difference from `TokenListings`**, which reads the raw
    // ledger and is lax on purpose. Over-fetching a token whose listing was
    // retracted costs one wasted ERC-20 read; over-fetching a *reserve* puts an
    // id the Spoke no longer knows about into a batch call, and
    // `getReservesPrices` would revert on it and take the others with it.
    await index(100, 100, [
      addReserve({ block: 100, log: 0 }, '0', '1', HUB),
      addReserve({ block: 100, log: 1 }, '1', '2', HUB),
    ]);

    await index(100, 100, [addReserve({ block: 100, log: 0 }, '0', '1', HUB)]);

    expect(await listings.forSpoke(CHAIN_ID, SPOKE)).toEqual(['0']);
  });

  it('is empty before the Spoke has registered anything', async () => {
    // A cold start, which the processor treats as "not yet" rather than as a
    // failure — the `AddReserve` events have not been folded, and they are
    // about to be.
    expect(await listings.forSpoke(CHAIN_ID, SPOKE)).toEqual([]);
  });

  it('takes the spoke as given, cased or not', async () => {
    await index(100, 100, [addReserve({ block: 100, log: 0 }, '0', '1', HUB)]);

    expect(await listings.forSpoke(CHAIN_ID, '0x94E7A5dCbE816e498b89aB752661904E2F56c485')).toEqual(
      ['0'],
    );
  });

  it('is scoped to one chain', async () => {
    await index(100, 100, [addReserve({ block: 100, log: 0 }, '0', '1', HUB)]);

    expect(await listings.forSpoke(8453, SPOKE)).toEqual([]);
  });
});
