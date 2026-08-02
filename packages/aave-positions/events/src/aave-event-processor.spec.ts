import {
  LogRangeTooLargeError,
  type LogFilter,
  type LogReader,
  type RawLog,
} from '@packages/indexing';
import { encodeAbiParameters, encodeEventTopics } from 'viem';
import { describe, expect, it } from 'vitest';

import { HUB_ABI } from './aave/hub-events';
import { SPOKE_POSITION_TOPICS } from './aave/spoke-events';
import { AaveEventProcessor, hubEventSource, spokeEventSource } from './aave-event-processor';
import type { DecodedEvent } from './decode/decoded-event';
import type { EventStore } from './store/event-store';
import fixture from './decode/spoke-logs.fixture.json';

const CHAIN_ID = 1;
const SPOKE = fixture.provenance.spoke;
const REAL_LOGS = fixture.real as unknown as RawLog[];

/** Records the sequence, not just the effect: the order of the two is the contract. */
type StoreCall =
  | { readonly kind: 'revert'; readonly from: number; readonly to: number }
  | { readonly kind: 'append'; readonly count: number };

class RecordingStore implements EventStore {
  readonly calls: StoreCall[] = [];

  revert(_chainId: number, fromBlock: number, toBlock: number): Promise<void> {
    this.calls.push({ kind: 'revert', from: fromBlock, to: toBlock });
    return Promise.resolve();
  }

  append(events: readonly DecodedEvent[]): Promise<void> {
    this.calls.push({ kind: 'append', count: events.length });
    return Promise.resolve();
  }
}

class StubReader implements LogReader {
  readonly filters: LogFilter[] = [];

  constructor(private readonly result: RawLog[] | Error) {}

  getLogs(filter: LogFilter): Promise<RawLog[]> {
    this.filters.push(filter);
    return this.result instanceof Error
      ? Promise.reject(this.result)
      : Promise.resolve(this.result);
  }
}

function build(reader: LogReader, store: EventStore = new RecordingStore()) {
  return new AaveEventProcessor(spokeEventSource(CHAIN_ID, SPOKE), reader, store);
}

const running = new AbortController().signal;

describe('AaveEventProcessor', () => {
  it('asks only for the eight position topics on the configured spoke', async () => {
    const reader = new StubReader([]);

    await build(reader).onBlockRange(100, 200, running);

    expect(reader.filters).toEqual([
      {
        addresses: [SPOKE],
        topic0: SPOKE_POSITION_TOPICS,
        fromBlock: 100,
        toBlock: 200,
      },
    ]);
  });

  it('decodes and stores the range it was given', async () => {
    const store = new RecordingStore();

    const outcome = await build(new StubReader(REAL_LOGS), store).onBlockRange(100, 200, running);

    expect(outcome).toEqual({ status: 'ok' });
    // Revert then append, in that order. Dispatch is at-least-once and the
    // engine collapses no repeated insert on its own, so an append that got
    // ahead of the revert would leave two live copies and double every count.
    expect(store.calls).toEqual([
      { kind: 'revert', from: 100, to: 200 },
      { kind: 'append', count: REAL_LOGS.length },
    ]);
  });

  it('still writes an empty range, so the cursor advances through quiet stretches', async () => {
    const store = new RecordingStore();

    const outcome = await build(new StubReader([]), store).onBlockRange(100, 200, running);

    expect(outcome).toEqual({ status: 'ok' });
    // The write matters: it retracts anything a previous attempt left behind.
    expect(store.calls).toEqual([
      { kind: 'revert', from: 100, to: 200 },
      { kind: 'append', count: 0 },
    ]);
  });

  it('asks the loop to narrow when the provider rejects the span', async () => {
    const store = new RecordingStore();
    const reader = new StubReader(new LogRangeTooLargeError(100, 200, 'range too large'));

    const outcome = await build(reader, store).onBlockRange(100, 200, running);

    // This is what the framework's narrowRange flag was built for, and the
    // first thing that actually produces it — measured provider caps run from
    // 50 blocks to 50,000.
    expect(outcome).toMatchObject({ status: 'retry', narrowRange: true });
    expect(store.calls).toEqual([]);
  });

  it('fails terminally on a log it cannot decode', async () => {
    const foreign: RawLog = { ...REAL_LOGS[0]!, address: `0x${'99'.repeat(20)}` };
    const store = new RecordingStore();

    const outcome = await build(new StubReader([foreign]), store).onBlockRange(100, 200, running);

    // Not a retry: we asked for exactly these topics on exactly this address,
    // so this means the ABI or the filter is wrong and no attempt count helps.
    expect(outcome).toMatchObject({ status: 'failed' });
    expect(store.calls).toEqual([]);
  });

  it('rethrows an ordinary read failure for the loop to treat as retryable', async () => {
    const reader = new StubReader(new Error('connection refused'));

    await expect(build(reader).onBlockRange(100, 200, running)).rejects.toThrow(
      'connection refused',
    );
  });

  it('does not start a write once shutdown has begun', async () => {
    const aborted = new AbortController();
    aborted.abort();
    const store = new RecordingStore();

    const outcome = await build(new StubReader(REAL_LOGS), store).onBlockRange(
      100,
      200,
      aborted.signal,
    );

    expect(outcome).toMatchObject({ status: 'retry' });
    expect(store.calls).toEqual([]);
  });

  it('retracts the range on a reorg and inserts nothing', async () => {
    const store = new RecordingStore();

    const outcome = await build(new StubReader([]), store).onReorg(150, 160);

    expect(outcome).toEqual({ status: 'ok' });
    // A reorg only takes rows away; there is nothing to put back.
    expect(store.calls).toEqual([{ kind: 'revert', from: 150, to: 160 }]);
  });

  it('names the spoke it follows, so two of them are distinguishable', () => {
    const other = new AaveEventProcessor(
      spokeEventSource(CHAIN_ID, `0x${'22'.repeat(20)}`),
      new StubReader([]),
      new RecordingStore(),
    );

    expect(build(new StubReader([])).name).not.toBe(other.name);
  });

  describe('telling a listener what it just wrote', () => {
    const HUB = '0xcca852bc40e560adc3b1cc58ca5b55638ce826c9';

    /**
     * A real `AddAsset` log: `assetId` and `underlying` indexed, `decimals` in
     * the data. Topics derived from the ABI rather than pasted, so a signature
     * change breaks this loudly instead of silently matching nothing.
     */
    function addAssetLog(underlying: `0x${string}`): RawLog {
      return {
        address: HUB,
        topics: encodeEventTopics({
          abi: HUB_ABI,
          eventName: 'AddAsset',
          args: { assetId: 1n, underlying },
        }) as RawLog['topics'],
        data: encodeAbiParameters([{ type: 'uint8' }], [6]),
        blockNumber: 24_722_784,
        blockHash: `0x${'ab'.repeat(32)}`,
        blockTimestamp: 1_785_000_000,
        transactionHash: `0x${'cd'.repeat(32)}`,
        transactionIndex: 0,
        logIndex: 0,
      };
    }

    it('hands a Hub listing to the listener, after the write', async () => {
      const USDC = '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48' as const;
      const store = new RecordingStore();
      const seen: (readonly string[])[] = [];
      const order: string[] = [];

      const source = hubEventSource(CHAIN_ID, HUB, (tokens) => {
        order.push('listener');
        seen.push(tokens);
      });
      const appending = new RecordingStore();
      appending.append = (events) => {
        order.push('append');
        return store.append(events);
      };

      const reader = new StubReader([addAssetLog(USDC)]);
      await new AaveEventProcessor(source, reader, appending).onBlockRange(1, 2, running);

      // After, not before: a listener reacts to what landed, and must never be
      // able to make ingestion report a write it did not do.
      expect(order).toEqual(['append', 'listener']);
      expect(seen).toEqual([[USDC]]);
    });

    it('says nothing when the range listed nothing', async () => {
      const seen: (readonly string[])[] = [];
      const source = hubEventSource(CHAIN_ID, HUB, (tokens) => seen.push(tokens));

      await new AaveEventProcessor(source, new StubReader([]), new RecordingStore()).onBlockRange(
        1,
        2,
        running,
      );

      // Which is every range after genesis, and the whole reason the consumer
      // can stop asking a database whether anything happened.
      expect(seen).toEqual([]);
    });

    it('ingests identically with no listener at all', async () => {
      const store = new RecordingStore();
      const reader = new StubReader([]);

      const outcome = await new AaveEventProcessor(
        hubEventSource(CHAIN_ID, HUB),
        reader,
        store,
      ).onBlockRange(1, 2, running);

      // Optional by design: an indexer serving no labels has no use for it, and
      // ingestion cannot behave differently because one is absent.
      expect(outcome).toEqual({ status: 'ok' });
      expect(store.calls).toEqual([
        { kind: 'revert', from: 1, to: 2 },
        { kind: 'append', count: 0 },
      ]);
    });
  });
});
