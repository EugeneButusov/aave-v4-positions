import { Inject, Injectable } from '@nestjs/common';
import type { PublicClient } from 'viem';

import {
  CHAIN_CLIENT_OPTIONS,
  type BlockHeader,
  type ChainClient,
  type ChainClientOptions,
} from './chain-client';
import { connect } from './transport';

/**
 * Adapts viem to the {@link ChainClient} port.
 *
 * This is the only file that sees viem's `bigint` block numbers; the conversion
 * to `number` stops here so nothing downstream has to think about it.
 */
@Injectable()
export class ViemChainClient implements ChainClient {
  /** `protected` for {@link ViemLogReader}; nothing outside `chain/` reaches it. */
  protected readonly client: PublicClient;

  constructor(@Inject(CHAIN_CLIENT_OPTIONS) options: ChainClientOptions) {
    this.client = connect(options);
  }

  getChainId(): Promise<number> {
    return this.client.getChainId();
  }

  /**
   * `cacheTime: 0` is load-bearing. viem caches the block number for
   * `cacheTime` (defaulting to the 4s polling interval), so the default would
   * hand the loop a stale head and make it idle through blocks it could have
   * been indexing.
   */
  async getHeadBlockNumber(): Promise<number> {
    return Number(await this.client.getBlockNumber({ cacheTime: 0 }));
  }

  async getBlockHeader(blockNumber: number): Promise<BlockHeader> {
    const block = await this.client.getBlock({
      blockNumber: BigInt(blockNumber),
      includeTransactions: false,
    });

    return {
      number: Number(block.number),
      hash: block.hash,
      parentHash: block.parentHash,
      timestamp: Number(block.timestamp),
    };
  }
}
