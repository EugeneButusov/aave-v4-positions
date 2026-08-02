import { ISpokeV4_ABI } from '@aave-dao/aave-address-book/abis';
import type { Hash, RawLog } from '@packages/indexing';
import { encodeAbiParameters, encodeEventTopics, getAbiItem } from 'viem';
import { describe, expect, it } from 'vitest';

import { HUB_STATE_EVENTS } from '../aave/hub-events';
import { UndecodableLogError } from './decoder';
import { HubEventDecoder } from './hub-event-decoder';
import fixture from './hub-logs.fixture.json';

/**
 * `encodeEventTopics` types a topic as possibly null, for the wildcard form
 * where an indexed argument is left unconstrained. Every call here supplies all
 * of them, so nothing is null in practice.
 */
function topicsOf(encoded: readonly (Hash | readonly Hash[] | null)[]): Hash[] {
  return encoded.filter((topic): topic is Hash => typeof topic === 'string');
}

const CHAIN_ID = 1;
const HUB = fixture.provenance.hub;

const logs = [...fixture.real, ...fixture.synthetic] as unknown as RawLog[];
const decoder = new HubEventDecoder(CHAIN_ID, HUB);
const decoded = decoder.decode(logs);

function only(eventName: string) {
  const match = decoded.find((e) => e.eventName === eventName);
  if (!match) throw new Error(`fixture has no ${eventName}`);
  return match;
}

/** A Spoke log, which the Hub decoder must not accept even from the Hub address. */
function spokeLog(): RawLog {
  const item = getAbiItem({ abi: ISpokeV4_ABI, name: 'ReportDeficit' });
  const user: `0x${string}` = '0x82d16ff1c724ab72f218a3f7f6dd3e5385ee87e8';
  const args = {
    reserveId: 7n,
    user,
    drawnShares: 1n,
    premiumDelta: { sharesDelta: 0n, offsetRayDelta: 0n, restoredPremiumRay: 0n },
  };
  const nonIndexed = item.inputs.filter((i) => !i.indexed);

  return {
    address: HUB,
    topics: topicsOf(encodeEventTopics({ abi: ISpokeV4_ABI, eventName: 'ReportDeficit', args })),
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

describe('HubEventDecoder', () => {
  it('decodes every asset-state event', () => {
    // Real mainnet logs for the seven that fire there. The other six —
    // Sweep, Reclaim, MintFeeShares, RefreshPremium, ReportDeficit and
    // EliminateDeficit — have never fired on any of the four hubs (§5.4), so
    // they are ABI-encoded rather than found.
    expect(new Set(decoded.map((e) => e.eventName))).toEqual(new Set(HUB_STATE_EVENTS));
  });

  it('reads the interest checkpoint whole', () => {
    const update = only('UpdateAsset');

    // §5.3: the emitted index is the *settled* one, and the block's timestamp
    // is what makes (drawnIndex, drawnRate, t) a self-consistent checkpoint.
    // All three have to survive decoding for the fold to be able to use it.
    expect(update.body).toMatchObject({
      assetId: expect.any(String),
      drawnIndex: expect.any(String),
      drawnRate: expect.any(String),
      accruedFees: expect.any(String),
    });
    expect(update.blockTimestamp).toBeGreaterThan(0);
    // RAY-scaled, so it starts at 1e27 and only grows — already past 2^53 on
    // day one, which is why it is carried as a string (§7.5). The checkpoint
    // the fixture picks is the genesis one, where it is exactly RAY.
    expect(BigInt(String(update.body['drawnIndex']))).toBeGreaterThanOrEqual(10n ** 27n);
  });

  it('keeps the only source of underlying and decimals', () => {
    const added = only('AddAsset');

    // Nowhere else in either ABI. §12.2.
    expect(added.body).toMatchObject({
      assetId: expect.any(String),
      underlying: expect.stringMatching(/^0x[0-9a-fA-F]{40}$/),
      decimals: expect.any(Number),
    });
  });

  it('carries liquidityFee, which the supply-side fee term needs', () => {
    // The one field of the config tuple the valuation reads. Without it
    // `unrealizedFees` cannot be computed and every supply amount is too high.
    expect(only('UpdateAssetConfig').body['config']).toMatchObject({
      liquidityFee: expect.any(Number),
    });
  });

  it('keeps a signed premium delta signed', () => {
    // offsetRayDelta is int256 and the fixture sets it negative. A decoder that
    // widened it to unsigned would turn a small negative into ~1e77.
    expect(only('RefreshPremium').body['premiumDelta']).toMatchObject({
      sharesDelta: '1000000000000000000',
      offsetRayDelta: '-1000000000000000000000000000',
    });
  });

  it('distinguishes the calling spoke from the covered one', () => {
    const eliminated = only('EliminateDeficit');

    // Three indexed parameters, and the third is the spoke whose shares are
    // burned — the one the deficit belongs to. Reading topic2 for it would
    // credit the wrong spoke (§12.3).
    expect(eliminated.topic2).not.toBe(eliminated.topic3);
    expect(eliminated.topic3?.endsWith('94e7a5dcbe816e498b89ab752661904e2f56c485')).toBe(true);
  });

  it('puts the asset id in topic1 on all thirteen', () => {
    // The one column that means the same thing across the whole ledger — and a
    // different thing than it means in spoke_events, where it is a reserve id.
    for (const event of decoded) {
      expect(event.topic1).toMatch(/^0x0{40,}[0-9a-f]+$/);
    }
  });

  it('refuses a Spoke event, even emitted from the Hub address', () => {
    // §4.5's trap, from the other side. Spoke `ReportDeficit` is a different
    // event with a different topic0, and decoding by name across a merged
    // stream is how one silently becomes the other.
    expect(() => decoder.decode(spokeLog() ? [spokeLog()] : [])).toThrow(UndecodableLogError);
  });

  it('refuses a log from any other address', () => {
    const elsewhere = { ...logs[0], address: `0x${'99'.repeat(20)}` } as RawLog;

    expect(() => decoder.decode([elsewhere])).toThrow(/not the configured hub/);
  });

  it('lower-cases the address it was configured with', () => {
    const checksummed = new HubEventDecoder(CHAIN_ID, '0xCca852Bc40e560adC3b1Cc58CA5b55638ce826c9');

    // Log addresses arrive lower-cased; a checksummed configuration would match
    // none of them and reject the entire stream.
    expect(checksummed.decode([logs[0] as RawLog])[0]?.address).toBe(HUB);
  });
});
