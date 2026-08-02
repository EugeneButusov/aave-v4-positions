import { Inject, Injectable, NotFoundException } from '@nestjs/common';
import { POSITION_STORE, type Position, type PositionStore } from '@aave-positions/positions';
import { SYNC_STATUS_STORE, type SyncStatus, type SyncStatusStore } from '@packages/indexing';

import { PositionCursors, type CursorScope } from './position-cursors';
import type { PositionPageDto, PositionDto, SyncDto } from './positions.dto';
import type { PositionParams, PositionQueryParams } from './positions.schema';

export const STALE_AFTER_SECONDS = Symbol('STALE_AFTER_SECONDS');

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
 * It builds the {@link CursorScope} but never handles a cursor's bytes;
 * {@link PositionCursors} owns the wire format and what a bad one means.
 */
@Injectable()
export class PositionsService {
  constructor(
    @Inject(POSITION_STORE) private readonly positions: PositionStore,
    @Inject(SYNC_STATUS_STORE) private readonly sync: SyncStatusStore,
    @Inject(STALE_AFTER_SECONDS) private readonly staleAfterSeconds: number,
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

    const page = await this.positions.list({
      chainId: params.chainId,
      user: params.user,
      ...(query.spoke !== undefined && { spoke: query.spoke }),
      limit: query.limit,
      ...(query.asOf !== undefined && { asOf: BigInt(query.asOf) }),
      ...(query.cursor !== undefined && { after: this.cursors.decode(query.cursor, scope) }),
    });

    return {
      sync: toSync(sync, this.staleAfterSeconds),
      valuedAt: page.valuedAt,
      items: page.items.map(toPosition),
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

function toPosition(position: Position): PositionDto {
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
          },
  };
}
