import { createPublicClient, fallback, http, type PublicClient } from 'viem';

import type { ChainClientOptions } from './chain-client';

/**
 * Builds the transport from the configured provider list.
 *
 * Package-internal, and called exactly once: `ViemLogReader` extends
 * `ViemChainClient`, so there is one instance and one transport behind both
 * ports rather than two that could fail over independently.
 *
 * Does no I/O — neither `createPublicClient` nor `http()` opens a connection.
 * That is what lets an adapter be constructed during DI without making
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
export function connect(options: ChainClientOptions): PublicClient {
  return createPublicClient({
    transport: fallback(
      options.rpcUrls.map((url) => http(url, { timeout: options.rpcTimeoutMs })),
      { retryCount: 0, rank: false },
    ),
  });
}
