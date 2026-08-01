import { Injectable } from '@nestjs/common';

import type { BlockHeader } from '../../chain/chain-client';
import type { BlockHeaderStore } from './block-header-store';

/**
 * Keeps the retention window in process memory.
 *
 * It does not survive a restart, which for this port is not merely a missing
 * feature: with an empty window the detector has only `Cursor.lastHash` to go
 * on, so a fork that happened while the process was down is reported as
 * unrecoverable rather than repaired. That costs nothing today, because the
 * cursor is in memory too and there is no resume to protect — but a durable
 * cursor store paired with this adapter would be strictly worse than either
 * alone. The two want to land together.
 *
 * One sorted array per chain rather than a `Map` keyed by block number: the
 * window is read top-down and returned in order, and re-`set`ting an existing
 * key would keep its original insertion position, so a re-committed block after
 * a fork would silently sit in the wrong place.
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
