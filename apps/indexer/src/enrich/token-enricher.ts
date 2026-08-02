import { Inject, Injectable, Logger } from '@nestjs/common';
import {
  TOKEN_LISTINGS,
  TOKEN_METADATA_STORE,
  type TokenListings,
  type TokenMetadataStore,
} from '@aave-positions/enrichment';
import {
  CHAIN_CLIENT,
  ERC20_METADATA_READER,
  type Address,
  type ChainClient,
  type Erc20MetadataReader,
} from '@packages/indexing';

import type { EnrichRequest } from './enrich-args';

export interface EnrichResult {
  readonly asked: number;
  readonly resolved: number;
  /** Tokens that could not be reached, so their gap is still open. */
  readonly unreachable: readonly Address[];
}

/**
 * One pass of enrichment, run by hand.
 *
 * Shares the ports the processor uses rather than the processor itself: the
 * scheduling — the latch, the fast path, the swallow-and-carry-on — is exactly
 * what a command should not inherit. Here a failure is the operator's to see.
 */
@Injectable()
export class TokenEnricher {
  private readonly logger = new Logger(TokenEnricher.name);

  constructor(
    @Inject(TOKEN_LISTINGS) private readonly listings: TokenListings,
    @Inject(TOKEN_METADATA_STORE) private readonly store: TokenMetadataStore,
    @Inject(ERC20_METADATA_READER) private readonly reader: Erc20MetadataReader,
    @Inject(CHAIN_CLIENT) private readonly chain: ChainClient,
  ) {}

  async run(chainId: number, request: EnrichRequest): Promise<EnrichResult> {
    const listed = (await this.listings.all(chainId)).map((token) => token.toLowerCase());
    const known = await this.store.labels(chainId);

    const wanted =
      request.token === null
        ? listed.filter((token) => request.force || !known.has(token))
        : [request.token];

    if (request.token !== null && !listed.includes(request.token)) {
      // Read it anyway — an operator naming an address is usually chasing
      // something the ledger does not know about yet — but say so, because the
      // row will not join to anything until the Hub lists it.
      this.logger.warn(`${request.token} is not listed by the Hub on chain ${chainId}`, 'Enrich');
    }

    const head = BigInt(await this.chain.getHeadBlockNumber());
    const unreachable: Address[] = [];
    let resolved = 0;

    for (const token of wanted) {
      // Sequential, for the reason `reconcile-hub` gives: a public endpoint
      // rate-limits a burst of these long before seventeen calls become slow,
      // and this runs once.
      // oxlint-disable-next-line no-await-in-loop
      const metadata = await this.reader.read(token, head);

      if (metadata.failures.length > 0) {
        this.logger.warn(`${token}: ${metadata.failures.join(', ')}`, 'Enrich');
      }
      if (metadata.symbol === null && metadata.name === null && metadata.decimals === null) {
        unreachable.push(token);
        continue;
      }

      // oxlint-disable-next-line no-await-in-loop
      await this.store.put([
        {
          chainId,
          token,
          symbol: metadata.symbol,
          name: metadata.name,
          tokenDecimals: metadata.decimals,
          fetchedAtBlock: Number(head),
        },
      ]);
      resolved += 1;
      this.logger.log(
        `${token}  ${metadata.symbol ?? '—'}  (${metadata.decimals ?? '—'} decimals)`,
        'Enrich',
      );
    }

    return { asked: wanted.length, resolved, unreachable };
  }
}
