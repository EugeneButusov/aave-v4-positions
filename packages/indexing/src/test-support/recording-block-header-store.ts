import type { BlockHeader } from '../chain/chain-client';
import { InMemoryBlockHeaderStore } from '../indexing/reorg/in-memory-block-header-store';

/**
 * A retention window that records every write and can be made to fail on a
 * chosen one.
 *
 * The counterpart to `RecordingCursorStore.failSave`, and it exists for the same
 * reason: a durable window can refuse a write, and what the detector does then
 * decides whether the range is simply retried or whether the window and the
 * cursor part company for good. In-memory, neither call could fail, so those
 * paths had nothing to exercise them.
 */
export class RecordingBlockHeaderStore extends InMemoryBlockHeaderStore {
  readonly appended: BlockHeader[] = [];
  readonly truncatedAt: number[] = [];

  private failOnAppend: number | null = null;
  private failOnTruncate: number | null = null;
  private appendAttempts = 0;
  private truncateAttempts = 0;

  /** Make the `nth` append (1-based) reject. */
  failAppend(nth: number): this {
    this.failOnAppend = nth;
    return this;
  }

  /** Make the `nth` truncate (1-based) reject. */
  failTruncate(nth: number): this {
    this.failOnTruncate = nth;
    return this;
  }

  override async append(chainId: number, header: BlockHeader, retainFrom: number): Promise<void> {
    this.appendAttempts += 1;
    if (this.appendAttempts === this.failOnAppend) {
      throw new Error('header store unavailable');
    }

    await super.append(chainId, header, retainFrom);
    this.appended.push(header);
  }

  override async truncate(chainId: number, lastValidBlock: number): Promise<void> {
    this.truncateAttempts += 1;
    if (this.truncateAttempts === this.failOnTruncate) {
      throw new Error('header store unavailable');
    }

    await super.truncate(chainId, lastValidBlock);
    this.truncatedAt.push(lastValidBlock);
  }
}
