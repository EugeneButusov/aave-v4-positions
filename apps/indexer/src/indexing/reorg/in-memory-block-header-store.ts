import { Injectable } from '@nestjs/common';

import type { BlockHeader } from '../../chain/chain-client';
import type { BlockHeaderStore } from './block-header-store';

/**
 * Keeps the retention window in process memory.
 *
 * Not surviving a restart is more than a missing feature here: with an empty
 * window the detector has only `Cursor.lastHash`, so a fork that happened while
 * the process was down is reported unrecoverable rather than repaired. It costs
 * nothing while the cursor is in memory too — but pairing this adapter with a
 * durable cursor would be worse than either alone, so the two want to land
 * together.
 *
 * A sorted array per chain rather than a `Map`: re-`set`ting a key keeps its
 * original insertion position, so a block re-committed after a fork would
 * silently sit in the wrong place.
 */
@Injectable()
export class InMemoryBlockHeaderStore implements BlockHeaderStore {
  private readonly windows = new Map<number, BlockHeader[]>();

  load(chainId: number): Promise<BlockHeader[]> {
    // A copy: the window is the store's, and a caller that mutated what it read
    // would be writing through a read.
    return Promise.resolve([...(this.windows.get(chainId) ?? [])]);
  }

  append(chainId: number, header: BlockHeader, retainFrom: number): Promise<void> {
    // Dropping any existing entry at this height first is what makes the write
    // an upsert; the loop re-commits a block whose cursor save failed, and after
    // a fork it re-commits the same height with a different hash.
    const others = (this.windows.get(chainId) ?? []).filter(
      (entry) => entry.number !== header.number,
    );

    this.windows.set(
      chainId,
      [...others, header]
        .filter((entry) => entry.number >= retainFrom)
        .toSorted((left, right) => left.number - right.number),
    );
    return Promise.resolve();
  }

  truncate(chainId: number, lastValidBlock: number): Promise<void> {
    const window = this.windows.get(chainId);
    if (window) {
      this.windows.set(
        chainId,
        window.filter((entry) => entry.number <= lastValidBlock),
      );
    }
    return Promise.resolve();
  }
}
