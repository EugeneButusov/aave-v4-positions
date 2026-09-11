import type { ClickHouseClient } from '@clickhouse/client';
import {
  ClickHouseHubEventStore,
  ClickHouseSpokeEventStore,
  type DecodedEvent,
} from '@aave-positions/events';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import { ClickHousePositionStore } from './clickhouse-position-store';
import { RAY } from '../valuation/valuation';
import {
  HUB,
  HUB_TABLES,
  USDC,
  add,
  addAsset,
  draw,
  mintFeeShares,
  updateAsset,
} from '../test-support/hub-ledger';
import {
  ALICE,
  CHAIN_ID,
  SPOKE,
  TABLES,
  addReserve,
  borrow,
  migratedDatabase,
  setCollateral,
  supply,
} from '../test-support/spoke-ledger';

/** Its own database: sibling suites share table names and would truncate this one. */
const DATABASE = 'spec_position_valuation';

/** The checkpoint every fixture below extrapolates from. */
const CHECKPOINT_BLOCK = 100;
const CHECKPOINT_AT = 1_785_000_000 + CHECKPOINT_BLOCK;
const YEAR = 365 * 24 * 3600;
/** 5% per annum, RAY-scaled, as `drawnRate` arrives on `UpdateAsset`. */
const FIVE_PERCENT = (RAY / 20n).toString();

let client: ClickHouseClient;
let spokeEvents: ClickHouseSpokeEventStore;
let hubEvents: ClickHouseHubEventStore;
let store: ClickHousePositionStore;

const page = (over: Partial<Parameters<ClickHousePositionStore['list']>[0]> = {}) =>
  store.list({ chainId: CHAIN_ID, user: ALICE, spoke: SPOKE, limit: 100, ...over });

/**
 * A reserve that resolves all the way to a token, and a Hub asset with a
 * checkpoint — the state valuation needs before it can produce a number.
 *
 * Asset 7 borrows 400,000 of the 1,000,000 supplied, so the index actually
 * accrues: the short-circuit would hold it at RAY if nothing were drawn.
 */
async function listReserve(events: DecodedEvent[] = []): Promise<void> {
  await hubEvents.append([
    addAsset({ block: 10 }, USDC, 6),
    add({ block: 20 }, '1000000', '1000000'),
    draw({ block: 30 }, '400000', '400000'),
    updateAsset({ block: CHECKPOINT_BLOCK }, RAY.toString(), FIVE_PERCENT, '0'),
  ]);
  await spokeEvents.append([addReserve({ block: 10 }, '7', '7', HUB), ...events]);
}

describe('valuing a position', () => {
  beforeAll(async () => {
    client = await migratedDatabase(DATABASE);
    spokeEvents = new ClickHouseSpokeEventStore(client);
    hubEvents = new ClickHouseHubEventStore(client);
    store = new ClickHousePositionStore(client);
  });

  afterAll(async () => {
    await client.close();
  });

  beforeEach(async () => {
    for (const table of [...TABLES, ...HUB_TABLES])
      // oxlint-disable-next-line no-await-in-loop
      await client.command({ query: `TRUNCATE TABLE ${table}` });
  });

  describe('the registry', () => {
    it('resolves a reserve to its Hub asset and token', async () => {
      await listReserve([supply({ block: 200 }, ALICE, '7', '1000')]);

      // reserveId is a per-Spoke index and means nothing on its own (§1).
      // AddReserve gives it a Hub and an assetId; the Hub's AddAsset gives that
      // an ERC-20 and its decimals. Neither contract has both halves.
      expect((await page()).items[0]?.asset).toEqual({
        assetId: '7',
        hub: HUB,
        underlying: USDC,
        decimals: 6,
      });
    });

    it('reports null rather than zero for a reserve it has never seen', async () => {
      await spokeEvents.append([supply({ block: 200 }, ALICE, '99', '1000')]);

      // A zero here is indistinguishable from a real zero balance. The position
      // still appears, because its shares are real.
      const [position] = (await page()).items;
      expect(position?.suppliedShares).toBe('1000');
      expect(position?.asset).toBeNull();
      expect(position?.value).toBeNull();
    });

    it('reports null when the Hub has listed the asset but never checkpointed it', async () => {
      await hubEvents.append([addAsset({ block: 10 }, USDC, 6)]);
      await spokeEvents.append([
        addReserve({ block: 10 }, '7', '7', HUB),
        supply({ block: 200 }, ALICE, '7', '1000'),
      ]);

      // No UpdateAsset means no index, and without an index there is no
      // arithmetic to do — so no number is offered.
      expect((await page()).items[0]?.value).toBeNull();
    });
  });

  describe('the amounts', () => {
    it('turns supplied shares into a token amount', async () => {
      // Before the checkpoint, so the position exists at the instant valued at:
      // the fold is read as of that instant too, not just the index.
      await listReserve([supply({ block: 50 }, ALICE, '7', '1000')]);

      const [position] = (await page({ asOf: BigInt(CHECKPOINT_AT) })).items;
      // Valued at the checkpoint itself, so the index has not moved: the asset
      // holds 1,000,000 shares against 1,000,000 of underlying, and 1,000 shares
      // redeem for 1,000.
      expect(position?.value?.suppliedAmount).toBe('1000');
      expect(position?.value?.drawnIndex).toBe(RAY.toString());
    });

    it('grows a debt with time on a fixed share balance', async () => {
      await listReserve([borrow({ block: 50 }, ALICE, '7', '1000000')]);

      const now = await page({ asOf: BigInt(CHECKPOINT_AT) });
      const later = await page({ asOf: BigInt(CHECKPOINT_AT + YEAR) });

      // The whole reason a share balance is not a balance (§5): nothing was
      // indexed between these two reads.
      expect(now.items[0]?.value?.totalDebt).toBe('1000000');
      expect(later.items[0]?.value?.totalDebt).toBe('1050000');
      expect(now.items[0]?.drawnShares).toBe(later.items[0]?.drawnShares);
    });

    it('keeps the shares and the flow beside the amount', async () => {
      await listReserve([supply({ block: 200 }, ALICE, '7', '1000')]);

      // Cost basis and current value answer different questions, and the
      // difference between them is interest — so neither replaces the other.
      expect((await page()).items[0]).toMatchObject({
        suppliedShares: '1000',
        netSuppliedAmount: '1000',
        value: expect.objectContaining({ suppliedAmount: expect.any(String) }),
      });
    });

    it('values every position on a page at one instant', async () => {
      await listReserve([
        supply({ block: 200, log: 0 }, ALICE, '7', '1000'),
        borrow({ block: 200, log: 1 }, ALICE, '7', '500'),
      ]);

      const result = await page({ asOf: BigInt(CHECKPOINT_AT + YEAR) });

      // One instant for the whole page, reported back: an amount without the
      // moment it was computed at is not reproducible (§12.6).
      expect(result.valuedAt).toBe(CHECKPOINT_AT + YEAR);
      const indexes = new Set(result.items.map((p) => p.value?.drawnIndex));
      expect(indexes.size).toBe(1);
    });

    it('defaults to now when no instant is named', async () => {
      await listReserve([supply({ block: 200 }, ALICE, '7', '1000')]);

      const before = Math.floor(Date.now() / 1000);
      const result = await page();

      // Which is what the chain does — getUserDebt at `latest` extrapolates to
      // the head block rather than to the last event.
      expect(result.valuedAt).toBeGreaterThanOrEqual(before);
      expect(result.items[0]?.value?.drawnIndex).not.toBe(RAY.toString());
    });

    it('returns every amount as a string, exact past 2^53', async () => {
      await listReserve([borrow({ block: 50 }, ALICE, '7', '422166581625087607993')]);

      const [position] = (await page({ asOf: BigInt(CHECKPOINT_AT) })).items;
      expect(position?.value?.drawnDebt).toBe('422166581625087607993');
      expect(typeof position?.value?.totalDebt).toBe('string');
    });
  });

  /**
   * Which fold the page reads, as opposed to which checkpoint it values with.
   *
   * A share balance is the sum of every delta up to an instant, and until the
   * fold carried one it could only be summed up to *now* — so naming an `asOf`
   * moved the interest index and left the balances where they were. The two
   * halves have to agree or the page reports a debt that was never owed.
   */
  describe('the instant it reads the fold at', () => {
    /** Between the two events every case below writes. */
    const BETWEEN = BigInt(CHECKPOINT_AT + 150);
    const AFTER = BigInt(CHECKPOINT_AT + 250);

    it('returns the shares held then, not the ones held since', async () => {
      await listReserve([
        supply({ block: 200 }, ALICE, '7', '1000'),
        supply({ block: 300 }, ALICE, '7', '500'),
      ]);

      expect((await page({ asOf: BETWEEN })).items[0]?.suppliedShares).toBe('1000');
      expect((await page({ asOf: AFTER })).items[0]?.suppliedShares).toBe('1500');
    });

    it('values those shares rather than the ones held now', async () => {
      await listReserve([
        borrow({ block: 200 }, ALICE, '7', '1000000'),
        borrow({ block: 300 }, ALICE, '7', '1000000'),
      ]);

      // Both reads extrapolate the same checkpoint to their own instant, so the
      // gap between them is the second borrow and not interest.
      const before = await page({ asOf: BETWEEN });
      const after = await page({ asOf: AFTER });

      expect(before.items[0]?.drawnShares).toBe('1000000');
      expect(after.items[0]?.drawnShares).toBe('2000000');
      expect(BigInt(after.items[0]?.value?.totalDebt ?? '0')).toBeGreaterThan(
        2n * BigInt(before.items[0]?.value?.totalDebt ?? '0') - 10n,
      );
    });

    it('leaves out a position that did not exist yet', async () => {
      await listReserve([supply({ block: 300 }, ALICE, '7', '1000')]);

      // Its shares sum to nothing before its first event, and a position with no
      // shares is one the listing filter drops — the same rule that hides a
      // closed one.
      expect((await page({ asOf: BETWEEN })).items).toEqual([]);
      expect((await page({ asOf: AFTER })).items).toHaveLength(1);
    });

    it('reads the collateral flag as it stood', async () => {
      await listReserve([
        supply({ block: 200 }, ALICE, '7', '1000'),
        setCollateral({ block: 300 }, ALICE, '7', true),
      ]);

      // Latest-wins, so the cut changes which row argMax lands on rather than
      // whether it finds one. Without it the flag would read as set at instants
      // before anyone set it.
      expect((await page({ asOf: BETWEEN })).items[0]?.usingAsCollateral).toBe(false);
      expect((await page({ asOf: AFTER })).items[0]?.usingAsCollateral).toBe(true);
    });

    it('does not resolve a reserve listed after the instant', async () => {
      await hubEvents.append([
        addAsset({ block: 10 }, USDC, 6),
        add({ block: 20 }, '1000000', '1000000'),
        draw({ block: 30 }, '400000', '400000'),
        updateAsset({ block: CHECKPOINT_BLOCK }, RAY.toString(), FIVE_PERCENT, '0'),
      ]);
      await spokeEvents.append([
        supply({ block: 200 }, ALICE, '7', '1000'),
        addReserve({ block: 300 }, '7', '7', HUB),
      ]);

      // The shares are real at both instants; what moves is whether the registry
      // can yet say which Hub asset they belong to. Null rather than resolved
      // through a listing that had not happened — the same answer as a reserve
      // this deployment has never seen.
      const before = (await page({ asOf: BETWEEN })).items[0];
      expect(before?.suppliedShares).toBe('1000');
      expect(before?.asset).toBeNull();
      expect(before?.value).toBeNull();

      expect((await page({ asOf: AFTER })).items[0]?.asset).not.toBeNull();
    });

    it('answers the same after later events land', async () => {
      await listReserve([supply({ block: 200 }, ALICE, '7', '1000')]);
      const before = await page({ asOf: BETWEEN });

      await spokeEvents.append([supply({ block: 300 }, ALICE, '7', '500')]);

      // §12.6's promise, and the half of it the fold owns: a page pinned to an
      // instant does not move because the indexer did.
      expect((await page({ asOf: BETWEEN })).items).toEqual(before.items);
    });
  });

  /**
   * Which checkpoint the extrapolation starts from.
   *
   * The index accrues per second and is emitted only when something touches the
   * asset, so a valuation is a checkpoint plus linear interest to the instant
   * asked for (§5.3). Which checkpoint is therefore the whole of the answer, and
   * it has to be the one in force then: a later one is a negative elapsed, which
   * reverts on chain and threw here, and one chosen by how far the indexer
   * happens to have got is a different answer to the same question each time.
   */
  describe('the checkpoint it extrapolates from', () => {
    const LATER_BLOCK = CHECKPOINT_BLOCK + 200;
    const LATER_AT = 1_785_000_000 + LATER_BLOCK;
    /** An index no extrapolation from the first checkpoint could reach. */
    const DOUBLED = (RAY * 2n).toString();

    async function twoCheckpoints(): Promise<void> {
      await listReserve([supply({ block: 50 }, ALICE, '7', '1000')]);
      await hubEvents.append([updateAsset({ block: LATER_BLOCK }, DOUBLED, FIVE_PERCENT, '0')]);
    }

    it('takes the one in force at the instant, not the newest there is', async () => {
      await twoCheckpoints();

      // The newest checkpoint is 200s after this instant, and reaching it from
      // here is `calculateLinearInterest` over a negative elapsed — which the
      // arithmetic refuses, so the whole page failed rather than valued.
      expect((await page({ asOf: BigInt(CHECKPOINT_AT) })).items[0]?.value?.drawnIndex).toBe(
        RAY.toString(),
      );
    });

    it('carries it forward to the instant rather than snapping to it', async () => {
      await twoCheckpoints();
      const elapsed = 100n;

      // Still the earlier checkpoint, and 100s of interest on top: the cut
      // selects a base, it does not replace the extrapolation.
      expect(
        (await page({ asOf: BigInt(CHECKPOINT_AT) + elapsed })).items[0]?.value?.drawnIndex,
      ).toBe((RAY + (BigInt(FIVE_PERCENT) * elapsed) / BigInt(YEAR)).toString());
    });

    it('moves to the later one once the instant reaches it', async () => {
      await twoCheckpoints();

      expect((await page({ asOf: BigInt(LATER_AT) })).items[0]?.value?.drawnIndex).toBe(DOUBLED);
    });

    it('reports null when no checkpoint precedes the instant', async () => {
      await twoCheckpoints();

      // Nothing to carry forward, so no number is offered — the same answer as
      // an asset the Hub has listed and never checkpointed at all.
      const [position] = (await page({ asOf: BigInt(CHECKPOINT_AT) - 1n })).items;
      expect(position?.value).toBeNull();
    });

    it('answers the same after a checkpoint lands past the instant', async () => {
      await listReserve([supply({ block: 50 }, ALICE, '7', '1000')]);
      const asOf = BigInt(CHECKPOINT_AT) + 100n;
      const before = await page({ asOf });

      await hubEvents.append([updateAsset({ block: LATER_BLOCK }, DOUBLED, FIVE_PERCENT, '0')]);

      // §12.6's promise, and the half the Hub dimension owns.
      expect((await page({ asOf })).items[0]?.value).toEqual(before.items[0]?.value);
    });
  });

  /**
   * A reorg, read at an instant the retracted event was live at.
   *
   * Every other reorg case here reads the fold at now, where the sum is right
   * whatever instant the `+1` and its `-1` twin landed on — they cancel and the
   * total is the same. A cut does not have that luxury: if a retraction carried
   * the *replacement* block's instant rather than the retracted one's, the two
   * would sit on either side of a cut between them and the read would keep a
   * delta the chain no longer has.
   *
   * It cannot, because `revert` is `INSERT … SELECT` over the ledger's own rows
   * and `block_timestamp` is among the columns it copies. This is what says so.
   */
  describe('a reorg', () => {
    const BETWEEN = BigInt(CHECKPOINT_AT + 150);
    const AFTER = BigInt(CHECKPOINT_AT + 250);
    /** The two replacement checkpoints' own instants, where the index is the
     *  checkpoint itself rather than the checkpoint plus interest. */
    const AT_200 = 1_785_000_000 + 200;
    const AT_300 = 1_785_000_000 + 300;

    it('leaves no trace at an instant the retracted event was live at', async () => {
      await listReserve([supply({ block: 200 }, ALICE, '7', '1000')]);
      expect((await page({ asOf: BETWEEN })).items[0]?.suppliedShares).toBe('1000');

      // The chain replaced 200-300: the supply is smaller and lands later.
      await spokeEvents.revert(CHAIN_ID, 200, 300);
      await spokeEvents.append([supply({ block: 300 }, ALICE, '7', '500')]);

      // At an instant between the two branches the wallet holds nothing — the
      // old supply is retracted and the new one has not happened. A retraction
      // stamped with the new block's instant would read 1000 here.
      expect((await page({ asOf: BETWEEN })).items).toEqual([]);
      expect((await page({ asOf: AFTER })).items[0]?.suppliedShares).toBe('500');
    });

    it('does not price shares against a Hub total the chain no longer has', async () => {
      await listReserve([supply({ block: 50 }, ALICE, '7', '1000')]);
      const baseline = (await page({ asOf: BETWEEN })).items[0]?.value?.suppliedAmount;

      // Fee shares are minted against the same assets, so the supply side's
      // denominator grows and a share redeems for less. A Sweep would not do:
      // it moves liquidity into `swept` by the same amount, and
      // `totalAddedAssets` adds the two back together.
      await hubEvents.append([mintFeeShares({ block: 200 }, '500000', '500000')]);
      const diluted = (await page({ asOf: BETWEEN })).items[0]?.value?.suppliedAmount;
      expect(diluted).not.toBe(baseline);

      await hubEvents.revert(CHAIN_ID, 200, 300);
      await hubEvents.append([mintFeeShares({ block: 300 }, '500000', '500000')]);

      // The totals are a sum over every delta up to the instant, so a
      // retraction landing on the wrong side of the cut leaves the asset priced
      // against liquidity the chain no longer has.
      expect((await page({ asOf: BETWEEN })).items[0]?.value?.suppliedAmount).toBe(baseline);
      expect((await page({ asOf: BigInt(AT_300) })).items[0]?.value?.suppliedAmount).toBe(diluted);
    });

    it('does not extrapolate from a checkpoint the chain no longer has', async () => {
      await listReserve([supply({ block: 50 }, ALICE, '7', '1000')]);
      await hubEvents.append([
        updateAsset({ block: 200 }, (RAY * 2n).toString(), FIVE_PERCENT, '0'),
      ]);
      // At that checkpoint's own instant, so the index is it and not it plus
      // interest — `drawnIndexAt` short-circuits when the two coincide.
      expect((await page({ asOf: BigInt(AT_200) })).items[0]?.value?.drawnIndex).toBe(
        (RAY * 2n).toString(),
      );

      await hubEvents.revert(CHAIN_ID, 200, 300);
      await hubEvents.append([
        updateAsset({ block: 300 }, (RAY * 3n).toString(), FIVE_PERCENT, '0'),
      ]);

      // Back to the block-100 checkpoint carried forward 150s: the retracted one
      // is gone and its replacement has not happened. The collapse runs
      // `HAVING sum(sign) > 0` *after* the cut, so a pair split across it would
      // leave the retracted checkpoint standing and extrapolate every amount on
      // the page from an index the chain never had.
      expect((await page({ asOf: BETWEEN })).items[0]?.value?.drawnIndex).toBe(
        (RAY + (BigInt(FIVE_PERCENT) * 150n) / BigInt(YEAR)).toString(),
      );
      expect((await page({ asOf: BigInt(AT_300) })).items[0]?.value?.drawnIndex).toBe(
        (RAY * 3n).toString(),
      );
    });
  });
});
