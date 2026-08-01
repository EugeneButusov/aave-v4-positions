import { Inject, Injectable } from '@nestjs/common';
import { createPublicClient, fallback, http, type PublicClient } from 'viem';

import {
  CHAIN_CLIENT_OPTIONS,
  type BlockHeader,
  type ChainClient,
  type ChainClientOptions,
} from './chain-client';

/**
 * Builds the viem client from the configured provider list.
 *
 * Does no I/O — neither `createPublicClient` nor `http()` opens a connection.
 * That is what lets the adapter be constructed during DI without making
 * application boot depend on an RPC node being reachable: a pod comes up and
 * reports not-ready, rather than crash-looping while the provider is down.
 *
 * Three viem behaviours are worth stating, all verified against the 2.55 source
 * rather than the documentation:
 *
 * - **Per-provider retry is not ours to configure.** `fallback` re-invokes each
 *   inner transport with `retryCount: 0` on every request, so a `retryCount`
 *   passed to `http()` here would be dead configuration. One attempt per
 *   provider, then fall over — which is what we want anyway.
 * - **`fallback`'s own `retryCount` is set to 0.** It re-runs the *entire*
 *   provider list, and only once every provider has already failed. At viem's
 *   default of 3 a total outage costs four full sweeps before the caller hears
 *   about it — backoff the indexing loop cannot see and cannot factor into its
 *   own retry decision. Failing fast hands that judgement back to the loop.
 * - **`rank` is left at its default of `false`.** Ranking re-orders providers by
 *   measured latency and stability every 10s, which is right when the list is
 *   interchangeable. Here it is a stated preference order.
 *
 * The per-transport `timeout` does apply: `fallback` forwards the client-level
 * timeout, which is unset here, so `http()`'s own value wins.
 *
 * No `chain` is passed. It keeps viem's chain registry out of the build, works
 * against a local Anvil on 31337, and leaves the configured chain id as purely
 * the *expected* value — which is what {@link ViemChainClient.getChainId} is
 * checked against at runtime.
 */
function buildPublicClient(options: ChainClientOptions): PublicClient {
  return createPublicClient({
    transport: fallback(
      options.rpcUrls.map((url) => http(url, { timeout: options.rpcTimeoutMs })),
      { retryCount: 0, rank: false },
    ),
  });
}

/**
 * Adapts viem to the {@link ChainClient} port.
 *
 * The client is built from configuration here rather than injected: it is an
 * implementation detail of this adapter, and threading it through DI would make
 * the choice of RPC library part of the module's wiring. Everything outside this
 * file talks to the port.
 *
 * This is also the only file in the app that sees viem's `bigint` block numbers;
 * the conversion to `number` stops here so nothing downstream has to think about
 * it.
 */
@Injectable()
export class ViemChainClient implements ChainClient {
  private readonly client: PublicClient;

  constructor(@Inject(CHAIN_CLIENT_OPTIONS) options: ChainClientOptions) {
    this.client = buildPublicClient(options);
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
