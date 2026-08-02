import { Inject, Injectable, NotFoundException } from '@nestjs/common';
import {
  RESERVE_PRICE_STORE,
  TOKEN_METADATA_STORE,
  reserveKey,
  type ReservePrice,
  type ReservePriceStore,
  type TokenLabel,
  type TokenMetadataStore,
} from '@aave-positions/enrichment';
import {
  POSITION_STORE,
  toValue,
  type Position,
  type PositionStore,
} from '@aave-positions/positions';
import { SYNC_STATUS_STORE, type SyncStatus, type SyncStatusStore } from '@packages/indexing';

import { PositionCursors, type CursorScope } from './position-cursors';
import type { PositionPageDto, PositionDto, PricingDto, SyncDto } from './positions.dto';
import type { PositionParams, PositionQueryParams } from './positions.schema';

export const STALE_AFTER_SECONDS = Symbol('STALE_AFTER_SECONDS');
export const PRICE_STALE_AFTER_SECONDS = Symbol('PRICE_STALE_AFTER_SECONDS');

/** The USD half of a position, or nothing when there is no price behind it. */
interface Usd {
  readonly price: string;
  readonly suppliedValue: string;
  readonly debtValue: string;
}

const NO_PRICES: ReadonlyMap<string, ReservePrice> = new Map();

/**
 * Serves one wallet's positions.
 *
 * Thin, and the read side is why: the fold is materialized views, so there is
 * no ingestion to keep in step with and nothing to compute here. What this does
 * own is what the store deliberately does not — the listing a cursor belongs
 * to, the decision that a chain with no cursor row is a missing resource rather
 * than an empty list, and the mapping onto a wire contract free to change at a
 * different rate from the domain type.
 *
 * **It is also where the two enrichments are merged in.** Labels and prices are
 * fetched rather than folded, keyed by chain rather than by page, and joined
 * here rather than in SQL — which is what keeps `Position` and the ClickHouse
 * store unaware that either exists.
 *
 * It builds the {@link CursorScope} but never handles a cursor's bytes;
 * {@link PositionCursors} owns the wire format and what a bad one means.
 */
@Injectable()
export class PositionsService {
  constructor(
    @Inject(POSITION_STORE) private readonly positions: PositionStore,
    @Inject(TOKEN_METADATA_STORE) private readonly tokens: TokenMetadataStore,
    @Inject(RESERVE_PRICE_STORE) private readonly prices: ReservePriceStore,
    @Inject(SYNC_STATUS_STORE) private readonly sync: SyncStatusStore,
    @Inject(STALE_AFTER_SECONDS) private readonly staleAfterSeconds: number,
    @Inject(PRICE_STALE_AFTER_SECONDS) private readonly priceStaleAfterSeconds: number,
    private readonly cursors: PositionCursors,
  ) {}

  async list(params: PositionParams, query: PositionQueryParams): Promise<PositionPageDto> {
    // Read first, and fail here rather than after a query that would answer
    // "no positions" for a chain this deployment does not follow.
    const sync = await this.sync.get(params.chainId);
    if (sync === null) {
      throw new NotFoundException(
        `chain ${params.chainId} has not been indexed by this deployment`,
      );
    }

    const scope: CursorScope = {
      chainId: params.chainId,
      user: params.user,
      spoke: query.spoke ?? null,
    };

    // **An explicit `asOf` is served without prices at all**, and the read is
    // skipped rather than the result discarded. Amounts are extrapolated to
    // that instant; the stored price is whatever the oracle last said, which is
    // now. Pricing one against the other produces a number that was never true,
    // and the only honest alternative — a price series — is not what this
    // stores. Note the test is `undefined`, not falsy: the store defaults `asOf`
    // to now when the caller omits it, and that default is still "current".
    const wantsPrices = query.asOf === undefined;

    // **Side by side, not one after the other.** Both enrichments are keyed by
    // chain alone, so neither depends on which positions come back — which is
    // what lets a second database sit behind one response for free. Measured at
    // 0.27 ms against a 28 ms page, i.e. inside the noise.
    const [page, labels, prices] = await Promise.all([
      this.positions.list({
        chainId: params.chainId,
        user: params.user,
        ...(query.spoke !== undefined && { spoke: query.spoke }),
        limit: query.limit,
        ...(query.asOf !== undefined && { asOf: BigInt(query.asOf) }),
        ...(query.cursor !== undefined && { after: this.cursors.decode(query.cursor, scope) }),
      }),
      this.tokens.labels(params.chainId),
      wantsPrices ? this.prices.latest(params.chainId) : Promise.resolve(NO_PRICES),
    ]);

    const used = page.items
      .map((position) => priceFor(position, prices))
      .filter((price): price is ReservePrice => price !== undefined);

    return {
      sync: toSync(sync, this.staleAfterSeconds),
      valuedAt: page.valuedAt,
      pricing: toPricing(used, this.priceStaleAfterSeconds),
      items: page.items.map((position) =>
        toPosition(position, labels, usdFor(position, priceFor(position, prices))),
      ),
      nextCursor: page.next === null ? null : this.cursors.encode(scope, page.next),
    };
  }
}

function toSync(status: SyncStatus, staleAfterSeconds: number): SyncDto {
  return {
    lastBlock: status.lastBlock,
    lastBlockHash: status.lastHash,
    updatedAt: status.updatedAt.toISOString(),
    ageSeconds: status.ageSeconds,
    stale: status.ageSeconds > staleAfterSeconds,
  };
}

/**
 * The page's price clock, taken from the **oldest** price behind it.
 *
 * Not the newest and not an average: the question a caller has is how far to
 * trust the worst number in front of them. Prices are normally written in one
 * upsert and share a timestamp, so this only diverges when the oracle refused a
 * reserve and its last good price was left to age — which is exactly the case
 * worth surfacing.
 */
function toPricing(used: readonly ReservePrice[], staleAfterSeconds: number): PricingDto | null {
  const oldest = used.reduce<ReservePrice | null>(
    (worst, price) => (worst === null || price.ageSeconds > worst.ageSeconds ? price : worst),
    null,
  );

  if (oldest === null) return null;

  return {
    updatedAt: oldest.pricedAt.toISOString(),
    ageSeconds: oldest.ageSeconds,
    stale: oldest.ageSeconds > staleAfterSeconds,
  };
}

/** The price this position would be valued with, if there is one. */
function priceFor(
  position: Position,
  prices: ReadonlyMap<string, ReservePrice>,
): ReservePrice | undefined {
  // Gated on `value` as well as `asset`: with nothing to price, a price is not
  // "used" and must not drag the page's clock backwards.
  if (position.asset === null || position.value === null) return undefined;
  return prices.get(reserveKey(position.spoke, position.reserveId));
}

/**
 * One position's USD half.
 *
 * Null rather than zero when there is no price, for the reason `asset` and
 * `value` are null together: a zero here is indistinguishable from a position
 * genuinely worth nothing, and the oracle reverts rather than answer zero
 * (§7.4) so a real one cannot occur.
 */
function usdFor(position: Position, price: ReservePrice | undefined): Usd | null {
  if (price === undefined || position.asset === null || position.value === null) return null;

  // The **Hub's** decimals, from `AddAsset`, never the token's own `decimals()`.
  // Where the two disagree the Hub's is what the Hub's arithmetic uses, and so
  // what the position is worth to Aave.
  const { decimals } = position.asset;
  const answer = BigInt(price.price);

  return {
    price: price.price,
    suppliedValue: toValue(BigInt(position.value.suppliedAmount), decimals, answer).toString(),
    // `totalDebt`, which is rounded up into token units as the Spoke rounds a
    // repayment. That is the right number to display and the wrong one for a
    // health factor, which divides an unrounded ray-scaled debt — the two are
    // meant to differ in the last digits.
    debtValue: toValue(BigInt(position.value.totalDebt), decimals, answer).toString(),
  };
}

function toPosition(
  position: Position,
  labels: ReadonlyMap<string, TokenLabel>,
  usd: Usd | null,
): PositionDto {
  // Absent from the map means enrichment has not reached this token yet;
  // present with a null symbol means it was asked and has none. Both serve
  // null, because the wire cannot express the difference and a caller has no
  // use for it — but the store keeps them apart so the sweep knows what to do.
  const label = position.asset === null ? undefined : labels.get(position.asset.underlying);

  return {
    chainId: position.chainId,
    user: position.user,
    spoke: position.spoke,
    reserveId: position.reserveId,
    suppliedShares: position.suppliedShares,
    drawnShares: position.drawnShares,
    premiumShares: position.premiumShares,
    premiumOffsetRay: position.premiumOffsetRay,
    netSuppliedAmount: position.netSuppliedAmount,
    netBorrowedAmount: position.netBorrowedAmount,
    usingAsCollateral: position.usingAsCollateral,
    events: position.events,
    // Copied rather than spread, for the reason the DTOs exist at all: these
    // two are exactly where the domain type is still growing, and a spread
    // would publish the next field it gains without anyone deciding to.
    asset:
      position.asset === null
        ? null
        : {
            assetId: position.asset.assetId,
            hub: position.asset.hub,
            underlying: position.asset.underlying,
            decimals: position.asset.decimals,
            symbol: label?.symbol ?? null,
            name: label?.name ?? null,
          },
    value:
      position.value === null
        ? null
        : {
            suppliedAmount: position.value.suppliedAmount,
            drawnDebt: position.value.drawnDebt,
            premiumDebt: position.value.premiumDebt,
            totalDebt: position.value.totalDebt,
            drawnIndex: position.value.drawnIndex,
            price: usd?.price ?? null,
            suppliedValue: usd?.suppliedValue ?? null,
            debtValue: usd?.debtValue ?? null,
          },
  };
}
