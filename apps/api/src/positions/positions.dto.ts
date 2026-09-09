import { ApiProperty } from '@nestjs/swagger';

const EXAMPLE_WALLET = '0x82d16ff1c724ab72f218a3f7f6dd3e5385ee87e8';
const EXAMPLE_SPOKE = '0x94e7a5dcbe816e498b89ab752661904e2f56c485';
const EXAMPLE_HUB = '0xcca852bc40e560adc3b1cc58ca5b55638ce826c9';
const EXAMPLE_TOKEN = '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48';

/**
 * These are classes rather than interfaces because interfaces are erased before
 * runtime and OpenAPI schema generation reads decorator metadata. Every property
 * is annotated explicitly rather than relying on the `@nestjs/swagger` CLI
 * plugin: the plugin only runs through the Nest CLI build, so under Vitest the
 * generated document would silently lose properties and the contract test would
 * be asserting something the running service does not produce.
 *
 * They also mirror the domain types rather than reusing them. `Position` is free
 * to gain a field with the next ingestion increment; the wire contract is not,
 * and a shared type would move it without anyone deciding to.
 */
export class PositionAssetDto {
  @ApiProperty({
    example: '7',
    description: "The Hub's id for the asset, which is what makes it comparable across Spokes.",
  })
  assetId!: string;

  @ApiProperty({ example: EXAMPLE_HUB, description: 'The Hub holding the liquidity.' })
  hub!: string;

  @ApiProperty({
    example: EXAMPLE_TOKEN,
    description:
      'The ERC-20 itself. It appears in no Spoke event — resolving it needs the reserve ' +
      "registry and the Hub's own asset listing, which is why it is here rather than beside " +
      '`reserveId`.',
  })
  underlying!: string;

  @ApiProperty({ example: 6, description: 'Token decimals, as the Hub listed them.' })
  decimals!: number;

  @ApiProperty({
    type: String,
    nullable: true,
    example: 'USDC',
    description:
      "The token's own `symbol()`. **A label, not an identity** — nothing stops two tokens " +
      'claiming the same one, or claiming a familiar one in a different alphabet. `underlying` ' +
      'is the identity. Null when the token has no `symbol()`, which ERC-20 permits, and also ' +
      'in the window before enrichment has reached a newly listed asset.',
  })
  symbol!: string | null;

  @ApiProperty({
    type: String,
    nullable: true,
    example: 'USD Coin',
    description: "The token's own `name()`. Null on the same terms as `symbol`.",
  })
  name!: string | null;
}

export class PositionValueDto {
  @ApiProperty({
    example: '1000',
    description:
      'Underlying redeemable for the supplied shares, in whole tokens, rounded down as the ' +
      'Hub does.',
  })
  suppliedAmount!: string;

  @ApiProperty({ example: '0', description: 'Principal debt, rounded up as the Spoke does.' })
  drawnDebt!: string;

  @ApiProperty({ example: '0', description: 'Accrued risk premium, in whole tokens.' })
  premiumDebt!: string;

  @ApiProperty({ example: '0', description: '`drawnDebt + premiumDebt`.' })
  totalDebt!: string;

  @ApiProperty({
    example: '1.00113505584681013396716179',
    description:
      'The interest index this valuation used: the last checkpoint extrapolated to `valuedAt`, ' +
      'as a ray ratio where `1` is no accrual. Published so a caller can reproduce the ' +
      'arithmetic rather than trust it.',
  })
  drawnIndex!: string;

  @ApiProperty({
    type: String,
    nullable: true,
    example: '0.99971505',
    description:
      "What Aave's own oracle prices one whole token at, in dollars. This is the protocol's " +
      "view rather than the market's, which is the right one for a position: it is the " +
      'number that drives liquidation. Null when the oracle has not been read for this ' +
      'reserve yet, or when `asOf` is set — see `pricing`.',
  })
  priceUsd!: string | null;

  @ApiProperty({
    type: String,
    nullable: true,
    example: '999.71505',
    description:
      'What `suppliedAmount` is worth, in dollars. Every digit the protocol computed is kept ' +
      "— §7.1's unit puts `1e26` at one dollar, and this is that number divided rather than " +
      'rounded, so it still reconciles against `getUserAccountData` exactly. Null on the ' +
      'same terms as `priceUsd`.',
  })
  suppliedAmountUsd!: string | null;

  @ApiProperty({
    type: String,
    nullable: true,
    example: '0',
    description:
      'What `totalDebt` is worth, in dollars. Note this prices the **rounded** token amount, ' +
      'which is what is owed and what a caller should display. The health factor is computed ' +
      'from an unrounded ray-scaled debt instead, so the two will differ in the last digits ' +
      'by design. Null on the same terms as `priceUsd`.',
  })
  totalDebtUsd!: string | null;
}

export class PricingDto {
  @ApiProperty({
    example: '2026-08-02T11:04:17.000Z',
    description: 'When the oldest price behind any number on this page was read.',
  })
  updatedAt!: string;

  @ApiProperty({
    example: 41,
    description:
      "Seconds since then, measured by the database clock rather than this process's, so " +
      'clock skew between the two cannot be reported as staleness.',
  })
  ageSeconds!: number;

  @ApiProperty({
    example: false,
    description:
      "Whether that exceeds this deployment's threshold. It measures how long since **we** " +
      'last read the oracle, never how long since a feed last moved — an hour without a ' +
      'Chainlink update is normal behaviour, and a threshold set from feed cadence would ' +
      'flag healthy feeds forever.',
  })
  stale!: boolean;
}

export class PositionDto {
  @ApiProperty({ example: 1, description: 'The chain this position was indexed on.' })
  chainId!: number;

  @ApiProperty({
    example: EXAMPLE_WALLET,
    description:
      'The position owner, lower-cased. Never the caller that routed the action — ' +
      'position managers act on behalf of users, and crediting them would attribute ' +
      'large parts of the book to a handful of router addresses.',
  })
  user!: string;

  @ApiProperty({
    example: EXAMPLE_SPOKE,
    description:
      'The Spoke this position lives on. Always present, including when the request ' +
      'did not filter on one. Two Spokes are two isolated margin accounts with their ' +
      'own collateral factors, oracle and health factor, so positions on them may be ' +
      'listed together but must never be summed together.',
  })
  spoke!: string;

  @ApiProperty({
    example: '7',
    description:
      "The Spoke's own index for the reserve, and the only asset identity a position " +
      'has today. It is not a protocol-wide asset id and not a token address — no Spoke ' +
      'event carries one. The same id on two Spokes means two different things.',
  })
  reserveId!: string;

  @ApiProperty({
    type: String,
    nullable: true,
    example: '500.5',
    description:
      "Supplied balance, in shares, scaled by the asset's decimals. Not an asset amount — " +
      'see the endpoint description. **Null exactly when `asset` is**: the scale lives on the ' +
      'asset, so without it there is no honest way to render this, and an unscaled integer ' +
      'in a field documented as decimal would be wrong by orders of magnitude.',
  })
  suppliedShares!: string | null;

  @ApiProperty({
    type: String,
    nullable: true,
    example: '0',
    description: 'Borrowed balance, in shares. Null on the same terms as `suppliedShares`.',
  })
  drawnShares!: string | null;

  @ApiProperty({
    type: String,
    nullable: true,
    example: '0',
    description: 'Accrued risk premium, in shares. Null on the same terms as `suppliedShares`.',
  })
  premiumShares!: string | null;

  @ApiProperty({
    example: '0',
    description:
      'Premium offset, as a ray ratio. **Never null**, unlike the share fields beside it: ' +
      "a ray's scale is the protocol's fixed 27 rather than the asset's, so it can be " +
      'rendered whether or not the registry has resolved the reserve.',
  })
  premiumOffsetRay!: string;

  @ApiProperty({
    type: String,
    nullable: true,
    example: '500.5',
    description:
      'Net principal supplied, in asset units. A *flow*, not a balance: it sums what ' +
      'the events carried, and between events the interest index accrues while emitting ' +
      'nothing. Null on the same terms as `suppliedShares`.',
  })
  netSuppliedAmount!: string | null;

  @ApiProperty({
    type: String,
    nullable: true,
    example: '0',
    description:
      'Net principal borrowed, in asset units. Also a flow. Null on the same terms as ' +
      '`suppliedShares`.',
  })
  netBorrowedAmount!: string | null;

  @ApiProperty({
    example: true,
    description:
      "The user's own collateral flag, and only that. It is not the `collateral` position " +
      'type, which additionally requires a non-zero collateral factor under the version of ' +
      'the reserve config the user is pinned to — config events this build does not ingest. ' +
      "Five of the Main Spoke's fourteen reserves sit at a zero factor, so this flag alone " +
      'overstates collateral for those.',
  })
  usingAsCollateral!: boolean;

  @ApiProperty({
    example: 3,
    description: 'Ledger rows folded into this position. Never zero for a returned position.',
  })
  events!: number;

  @ApiProperty({
    type: PositionAssetDto,
    nullable: true,
    description:
      'What `reserveId` actually refers to, once the registry and the Hub have both been ' +
      'read. Null with `value` when the join has nothing to offer — a reserve the registry ' +
      'has not seen yet. Null rather than empty, because a blank token address is ' +
      'indistinguishable from a real one that failed to resolve.',
  })
  asset!: PositionAssetDto | null;

  @ApiProperty({
    type: PositionValueDto,
    nullable: true,
    description:
      'The shares above, converted to whole tokens at `valuedAt`. Null when `asset` is, and ' +
      'also when the Hub has listed the asset but not yet checkpointed its index — a zero ' +
      'there could not be told apart from a real zero balance.',
  })
  value!: PositionValueDto | null;
}

export class SyncDto {
  @ApiProperty({
    example: 25652535,
    description: 'The last block the indexer processed on this chain.',
  })
  lastBlock!: number;

  @ApiProperty({
    example: `0x${'ab'.repeat(32)}`,
    description: 'The hash the indexer saw at that height.',
  })
  lastBlockHash!: string;

  @ApiProperty({
    example: '2026-08-02T11:04:31.221Z',
    description: 'When the indexer last advanced, by the database clock that recorded it.',
  })
  updatedAt!: string;

  @ApiProperty({
    example: 7,
    description:
      'Seconds since the indexer last advanced. Measured on the server that wrote the ' +
      'timestamp, so it is not affected by clock skew between hosts.',
  })
  ageSeconds!: number;

  @ApiProperty({
    example: false,
    description:
      'Whether `ageSeconds` has passed this deployment’s freshness threshold. A stale ' +
      'response is still served — the numbers were true as of `lastBlock` — but they are ' +
      'behind the chain.',
  })
  stale!: boolean;
}

export class PositionPageDto {
  @ApiProperty({
    type: SyncDto,
    description:
      'How current this payload is. Present on every page, because amounts are per-block ' +
      'quantities and a page whose sync differs from the previous one is the honest signal ' +
      'that the indexer advanced mid-walk.',
  })
  sync!: SyncDto;

  @ApiProperty({
    example: '2026-08-02T11:04:17.000Z',
    description:
      'When every amount on this page was computed — one instant for the whole page, so two ' +
      'positions in it cannot disagree about the time. Defaults to now, which is the same ' +
      'choice the chain makes. Distinct from `sync.lastBlock`: the shares are as far as the ' +
      'indexer has folded, the amounts are those shares valued at this instant. ISO 8601, ' +
      'matching `sync.updatedAt` and `pricing.updatedAt` — the `asOf` **query** parameter ' +
      'that sets it is still Unix seconds, so round-tripping this value means converting it.',
  })
  valuedAt!: string;

  @ApiProperty({
    type: PricingDto,
    nullable: true,
    description:
      'How current the prices behind this page are — **a third clock**, beside `sync` and ' +
      '`valuedAt`, because prices have their own source and their own cadence. It reports ' +
      'the *oldest* price used on this page, so it answers "how far can I trust the worst ' +
      'number in front of me". Null when nothing here is priced, and null whenever `asOf` ' +
      'is set: amounts are extrapolated to that instant and prices are not, so a value ' +
      'mixing the two would be a number that never existed.',
  })
  pricing!: PricingDto | null;

  @ApiProperty({ type: [PositionDto] })
  items!: PositionDto[];

  @ApiProperty({
    type: String,
    nullable: true,
    example: null,
    description:
      'Hand back verbatim as `cursor` to fetch the next page. `null` is the last page. ' +
      'Opaque and signed: it is only valid for the listing that issued it, so changing the ' +
      'wallet, chain or Spoke filter while reusing it is refused rather than silently ' +
      'resuming somewhere else.',
  })
  nextCursor!: string | null;
}
