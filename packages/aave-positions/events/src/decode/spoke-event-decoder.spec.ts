import { IHubV4_ABI } from '@aave-dao/aave-address-book/abis';
import type { Hash, RawLog } from '@packages/indexing';
import { encodeAbiParameters, encodeEventTopics, getAbiItem } from 'viem';
import { describe, expect, it } from 'vitest';

import { SPOKE_POSITION_EVENTS } from '../aave/spoke-events';
import { UndecodableLogError } from './decoder';
import { SpokeEventDecoder } from './spoke-event-decoder';
import fixture from './spoke-logs.fixture.json';

/**
 * `encodeEventTopics` types a topic as possibly null, for the wildcard form
 * where an indexed argument is left unconstrained. Every call here supplies all
 * of them, so nothing is null in practice.
 */
function topicsOf(encoded: readonly (Hash | readonly Hash[] | null)[]): Hash[] {
  return encoded.filter((topic): topic is Hash => typeof topic === 'string');
}

/** A topic word: an address or a uint256, left-padded to 32 bytes as ABI encoding does. */
function padded(value: string): string {
  const hex = value.startsWith('0x') ? value.slice(2) : BigInt(value).toString(16);
  return `0x${hex.padStart(64, '0')}`.toLowerCase();
}

const CHAIN_ID = 1;
const SPOKE = fixture.provenance.spoke;

const logs = [...fixture.real, ...fixture.synthetic] as unknown as RawLog[];
const decoder = new SpokeEventDecoder(CHAIN_ID, SPOKE);
const decoded = decoder.decode(logs);

function only(eventName: string) {
  const match = decoded.find((e) => e.eventName === eventName);
  if (!match) throw new Error(`fixture has no ${eventName}`);
  return match;
}

/** Builds a log the Spoke decoder should not accept. */
function hubLog(): RawLog {
  const item = getAbiItem({ abi: IHubV4_ABI, name: 'ReportDeficit' });
  // Annotated so viem sees its own `0x${string}`. Arbitrary payload either way:
  // the log is rejected on its topic0, not on this field.
  const spoke: `0x${string}` = '0x94e7a5dcbe816e498b89ab752661904e2f56c485';
  const args = {
    assetId: 7n,
    spoke,
    drawnShares: 1n,
    premiumDelta: { sharesDelta: 0n, offsetRayDelta: 0n, restoredPremiumRay: 0n },
    deficitAmountRay: 0n,
  };
  const nonIndexed = item.inputs.filter((i) => !i.indexed);

  return {
    address: SPOKE,
    topics: topicsOf(encodeEventTopics({ abi: IHubV4_ABI, eventName: 'ReportDeficit', args })),
    data: encodeAbiParameters(
      nonIndexed,
      nonIndexed.map((i) => args[i.name as keyof typeof args]),
    ),
    blockNumber: 25_000_000,
    blockHash: `0x${'ab'.repeat(32)}`,
    blockTimestamp: 1_785_000_000,
    transactionHash: `0x${'cd'.repeat(32)}`,
    transactionIndex: 0,
    logIndex: 0,
  };
}

describe('SpokeEventDecoder', () => {
  it('decodes every position event', () => {
    // The fixture is real mainnet logs, save for LiquidationCall (90 in the
    // whole of history) and ReportDeficit (never emitted on mainnet), which are
    // ABI-encoded rather than found.
    expect(new Set(decoded.map((e) => e.eventName))).toEqual(new Set(SPOKE_POSITION_EVENTS));
  });

  it('attributes a routed action to the user, not the caller', () => {
    const routed = decoded.find(
      (e) =>
        e.blockNumber === fixture.routedCallerDiffersFromUser.blockNumber &&
        e.logIndex === fixture.routedCallerDiffersFromUser.logIndex,
    );

    // A real log where the two genuinely differ. Reading `caller` here would
    // book this position to a router address instead of its owner (§2), and
    // every one of the four hot events carries both, indexed. topic2 is the
    // caller and topic3 the user on these five, which is the ordering a
    // per-event view has to get right.
    expect(routed?.body['caller']).not.toEqual(routed?.body['user']);
    expect(routed?.topic2).toBe(padded(String(routed?.body['caller'])));
    expect(routed?.topic3).toBe(padded(String(routed?.body['user'])));
  });

  it('keeps Repay six fields wide, with its premium tuple intact', () => {
    // Repay is the odd one out: it inserts totalAmountRepaid before the
    // PremiumDelta tuple where the other three hot events have five flat
    // fields. §4.1 calls deriving it by hand an easy way to match nothing.
    expect(only('Repay').body).toMatchObject({
      reserveId: expect.any(String),
      caller: expect.any(String),
      user: expect.any(String),
      drawnShares: expect.any(String),
      totalAmountRepaid: expect.any(String),
      premiumDelta: {
        sharesDelta: expect.any(String),
        offsetRayDelta: expect.any(String),
        restoredPremiumRay: expect.any(String),
      },
    });
  });

  it('keeps the liquidator, which is not an indexed topic', () => {
    // §4.1: "positions liquidated by X" cannot be served by a topic filter, so
    // the liquidator has to survive into storage.
    expect(only('LiquidationCall').body['liquidator']).toBe(
      '0x2222222222222222222222222222222222222222',
    );
  });

  it('puts the indexed parameters in topics, in ABI order', () => {
    // topic1 is the reserve id on all eight — the collateral one where there
    // are two — while topic2 and topic3 mean different things per event. A
    // reader that ignores event_name gets the wrong field rather than nothing.
    expect(only('LiquidationCall').topic1).toBe(padded('7'));
    expect(only('LiquidationCall').topic2).toBe(padded('3'));
    expect(only('LiquidationCall').body).toMatchObject({
      collateralReserveId: '7',
      debtReserveId: '3',
    });

    // ReportDeficit indexes only two, so the third column is genuinely absent
    // rather than zero.
    expect(only('ReportDeficit').topic3).toBeNull();
  });

  it('renders every uint256 as a string', () => {
    // float64 has 53 bits of mantissa and share balances run far past it (§7.5).
    const supply = only('Supply');
    expect(typeof supply.body['suppliedShares']).toBe('string');
    expect(typeof supply.body['reserveId']).toBe('string');
    expect(() => JSON.stringify(supply.body)).not.toThrow();
  });

  it('preserves the raw envelope alongside the decoded body', () => {
    // What makes a decoder bug repairable by re-decoding rather than
    // re-fetching: topic0 is dropped because event_name says the same thing,
    // and the rest of the log survives verbatim.
    const source = logs[0];
    expect([decoded[0]?.topic1, decoded[0]?.topic2, decoded[0]?.topic3]).toEqual([
      source?.topics[1] ?? null,
      source?.topics[2] ?? null,
      source?.topics[3] ?? null,
    ]);
    expect(decoded[0]?.data).toEqual(source?.data);
  });

  it('accepts a checksummed spoke against lower-cased log addresses', () => {
    // Address books publish EIP-55 checksums and RPC returns lower case, so a
    // caller configuring the address straight from either source must work.
    // Without normalising, this rejects every log the Spoke ever emitted.
    const checksummed = new SpokeEventDecoder(
      CHAIN_ID,
      '0x94e7A5dCbE816e498b89aB752661904E2F56c485',
    );

    expect(checksummed.decode(logs)).toHaveLength(logs.length);
  });

  it('rejects a log from a different contract', () => {
    const foreign = { ...logs[0], address: `0x${'99'.repeat(20)}` } as RawLog;

    expect(() => decoder.decode([foreign])).toThrow(UndecodableLogError);
  });

  it('rejects a Hub ReportDeficit rather than decoding it as the Spoke one', () => {
    // The same event name exists on both contracts with different signatures
    // and different topic0s (§4.5). Decoding by topic0 across a merged stream
    // is how a Hub event silently becomes a wrong Spoke row.
    expect(() => decoder.decode([hubLog()])).toThrow(UndecodableLogError);
  });

  it('rejects a Spoke event that is not one of the eight requested', () => {
    const refresh: RawLog = {
      ...logs[0]!,
      topics: topicsOf(
        encodeEventTopics({
          abi: [
            {
              type: 'event',
              name: 'RefreshAllUserDynamicConfig',
              inputs: [{ name: 'user', type: 'address', indexed: true }],
            },
          ] as const,
          eventName: 'RefreshAllUserDynamicConfig',
          args: { user: '0x1111111111111111111111111111111111111111' },
        }),
      ),
      data: '0x',
    };

    // It decodes cleanly against the Spoke ABI — it is a real Spoke event — but
    // the filter never asked for it, so its arrival means the filter did not do
    // what it claimed. Silently keeping it would put unrequested rows in the
    // ledger; silently dropping it would hide the broken filter.
    expect(() => decoder.decode([refresh])).toThrow(/not one of the spoke events/);
  });
});
