import { describe, expect, it } from 'vitest';

import type { BlockHeader } from '../chain/chain-client';
import { describeBlockHeaderStoreContract } from './block-header-store-contract';
import { hashOf } from './fake-chain-client';
import { InMemoryBlockHeaderStore } from './in-memory-block-header-store';

/** No floor: keeps whatever is already retained. */
const KEEP_ALL = 0;

function header(blockNumber: number, branch = 'a'): BlockHeader {
  return {
    number: blockNumber,
    hash: hashOf(branch, blockNumber),
    parentHash: hashOf(branch, blockNumber - 1),
    timestamp: 1_700_000_000 + blockNumber * 12,
  };
}

describeBlockHeaderStoreContract('InMemoryBlockHeaderStore', {
  fresh: () => Promise.resolve(new InMemoryBlockHeaderStore()),
});

describe('InMemoryBlockHeaderStore — beyond the contract', () => {
  it('keeps its array sorted, though the port does not require it', async () => {
    const store = new InMemoryBlockHeaderStore();

    await store.append(1, header(502), KEEP_ALL);
    await store.append(1, header(500), KEEP_ALL);
    await store.append(1, header(501), KEEP_ALL);

    // An implementation detail, asserted here rather than in the contract
    // because it is not a promise the port makes — see ShufflingBlockHeaderStore,
    // which returns the same window reversed and is equally legal. It is worth
    // pinning anyway: the class holds an array precisely so a re-committed
    // height moves to its right position instead of keeping the slot a Map
    // would have left it in.
    expect((await store.load(1)).map((entry) => entry.number)).toEqual([500, 501, 502]);
  });
});
