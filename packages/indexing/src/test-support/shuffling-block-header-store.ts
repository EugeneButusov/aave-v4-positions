import type { BlockHeader } from '../chain/chain-client';
import { InMemoryBlockHeaderStore } from './in-memory-block-header-store';

/**
 * Hands the window back in the worst order it could. Stands in for an adapter
 * that reads rows without an `ORDER BY`, which the port permits.
 */
export class ShufflingBlockHeaderStore extends InMemoryBlockHeaderStore {
  override async load(chainId: number): Promise<BlockHeader[]> {
    return (await super.load(chainId)).toReversed();
  }
}
