import type { Cursor } from '../../src/indexing/cursor/cursor-store';
import { HashChainReorgDetector } from '../../src/indexing/reorg/hash-chain-reorg-detector';
import { InMemoryBlockHeaderStore } from '../../src/indexing/reorg/in-memory-block-header-store';
import type { IndexingOptions } from '../../src/indexing/indexing.options';
import { FakeChainClient, hashOf } from './fake-chain-client';

export const CHAIN_ID = 1;

export interface Harness {
  readonly detector: HashChainReorgDetector;
  readonly chain: FakeChainClient;
  readonly store: InMemoryBlockHeaderStore;
}

export function harness(finalityDepth = 10): Harness {
  const chain = new FakeChainClient({ head: 1_000 });
  const store = new InMemoryBlockHeaderStore();
  const detector = new HashChainReorgDetector(
    { chainId: CHAIN_ID, finalityDepth } as IndexingOptions,
    chain,
    store,
  );

  return { detector, chain, store };
}

/** Commits the chain's header for each block, as the chain reads it right now. */
export async function commit(
  { detector, chain }: Pick<Harness, 'detector' | 'chain'>,
  ...blockNumbers: number[]
): Promise<void> {
  for (const blockNumber of blockNumbers) {
    // Sequential on purpose: each commit prunes relative to its own height, so
    // the order they land in is the retention behaviour under test.
    // oxlint-disable-next-line no-await-in-loop
    await detector.commit(chain.headerAt(blockNumber));
  }
}

export function blocks(from: number, to: number): number[] {
  return Array.from({ length: to - from + 1 }, (_, index) => from + index);
}

export function cursorAt(lastBlock: number, branch = 'a'): Cursor {
  return { chainId: CHAIN_ID, lastBlock, lastHash: hashOf(branch, lastBlock) };
}

export async function retained({ store }: Harness): Promise<number[]> {
  return (await store.load(CHAIN_ID)).map((entry) => entry.number);
}
