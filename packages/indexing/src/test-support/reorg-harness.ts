import type { Cursor } from '../indexing/cursor/cursor-store';
import { HashChainReorgDetector } from '../indexing/reorg/hash-chain-reorg-detector';
import { InMemoryBlockHeaderStore } from '../indexing/reorg/in-memory-block-header-store';
import type { IndexingOptions } from '../indexing/indexing.options';
import { FakeChainClient, hashOf } from './fake-chain-client';

export const CHAIN_ID = 1;

export interface Harness {
  readonly detector: HashChainReorgDetector;
  readonly chain: FakeChainClient;
  readonly store: InMemoryBlockHeaderStore;
}

export const HEAD = 1_000;

export function harness(finalityDepth = 10, head = HEAD): Harness {
  const chain = new FakeChainClient({ head });
  const store = new InMemoryBlockHeaderStore();
  const detector = new HashChainReorgDetector(
    { chainId: CHAIN_ID, finalityDepth } as IndexingOptions,
    chain,
    store,
  );

  // The loop hands the head over on every iteration, so the detector always
  // knows where the boundary is by the time anything else is asked of it.
  // Priming keeps that true here rather than leaving it to fetch its own.
  detector.safeHead(head);

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
