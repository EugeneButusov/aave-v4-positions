import type { BlockHeader, ChainClient } from '../chain/chain-client';
import type { FakeChainClient } from './fake-chain-client';

/**
 * A chain that forks partway through a sequence of header reads, which is the
 * one thing {@link FakeChainClient} cannot script on its own — its forks apply
 * the moment they are declared, so every header it then serves is
 * self-consistent.
 */
export class ForkingChain implements ChainClient {
  private reads = 0;

  constructor(
    private readonly inner: FakeChainClient,
    private readonly afterReads: number,
    private readonly forkPoint: number,
  ) {}

  getChainId(): Promise<number> {
    return this.inner.getChainId();
  }

  getHeadBlockNumber(): Promise<number> {
    return this.inner.getHeadBlockNumber();
  }

  getBlockHeader(blockNumber: number): Promise<BlockHeader> {
    if (this.reads === this.afterReads) this.inner.forkAbove(this.forkPoint, 'b');
    this.reads += 1;
    return this.inner.getBlockHeader(blockNumber);
  }
}
