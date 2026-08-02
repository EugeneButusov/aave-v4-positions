import { Controller, Get, Param, Query } from '@nestjs/common';
import {
  ApiBadRequestResponse,
  ApiNotFoundResponse,
  ApiOkResponse,
  ApiOperation,
  ApiParam,
  ApiQuery,
  ApiTags,
} from '@nestjs/swagger';

import { ApiErrorDto } from '../common/api-error.dto';
import { ZodValidationPipe } from '../common/zod-validation.pipe';
import { PositionPageDto } from './positions.dto';
import {
  DEFAULT_PAGE_SIZE,
  GENESIS_UNIX_SECONDS,
  MAX_ASOF_UNIX_SECONDS,
  MAX_PAGE_SIZE,
  positionParamsSchema,
  positionQuerySchema,
  type PositionParams,
  type PositionQueryParams,
} from './positions.schema';
import { PositionsService } from './positions.service';

@ApiTags('positions')
@Controller({ path: 'chains/:chainId/users/:user/positions', version: '1' })
export class PositionsController {
  constructor(private readonly positions: PositionsService) {}

  @Get()
  @ApiOperation({
    summary: "List a wallet's positions",
    description: [
      'One wallet, on one chain, on one Spoke or on all of them. Ordered by Spoke then',
      'reserve, paged by keyset.',
      '',
      '**Every number is a decimal string, already scaled.** Amounts are in whole tokens,',
      '`priceUsd` and the `*Usd` fields are in dollars, and `drawnIndex` and',
      '`premiumOffsetRay` are ray ratios where `1` is no accrual. Nothing on the wire is in',
      'base units, so no caller has to know that a Value is `1e26` to the dollar or that a',
      'ray is `1e27` — but they are **strings**, and parsing one into a float undoes the',
      'point: a share balance of `422166581625087.607993` has 21 significant digits and a',
      'double keeps 17. Use a decimal type.',
      '',
      '**Balances are shares, not asset amounts.** A share is not a balance: the Hub index',
      'accrues every second and emits nothing, so `value` carries the shares converted at',
      '`valuedAt` and the top-level figures are what the events themselves said.',
      '`netSuppliedAmount` is a flow — what went in less what came out — and the difference',
      'between it and `value.suppliedAmount` is interest.',
      '',
      '**A field is null when the answer is unknown, never zero.** `asset` and `value` are',
      'null together when the registry has not resolved the reserve, and the share fields go',
      'with them, because the decimals that scale them live on the asset. The `*Usd` fields',
      'are null when the oracle has not priced the reserve, and whenever `asOf` is set. No',
      'health factor yet; the derivation is in `docs/aave-v4-protocol-analysis.md`.',
      '',
      '**Positions from different Spokes may be listed together but never summed.** Each',
      'Spoke is an isolated margin account with its own collateral factors, oracle and',
      'health factor, so a total across two of them hides an imminent liquidation behind',
      'unrelated collateral. Every row names the Spoke it came from.',
    ].join('\n'),
  })
  @ApiParam({ name: 'chainId', example: 1, description: 'The chain to read. Required.' })
  @ApiParam({
    name: 'user',
    example: '0x82d16ff1c724ab72f218a3f7f6dd3e5385ee87e8',
    description: 'The position owner. Checksummed or lower-case, either matches.',
  })
  @ApiQuery({
    name: 'spoke',
    required: false,
    example: '0x94e7a5dcbe816e498b89ab752661904e2f56c485',
    description: 'Narrow to one Spoke. Omit to list every Spoke this wallet has touched.',
  })
  @ApiQuery({
    name: 'limit',
    required: false,
    schema: { type: 'integer', minimum: 1, maximum: MAX_PAGE_SIZE, default: DEFAULT_PAGE_SIZE },
  })
  @ApiQuery({
    name: 'cursor',
    required: false,
    description:
      'A `nextCursor` from a previous page, handed back verbatim. Only valid for the ' +
      'listing that issued it.',
  })
  @ApiQuery({
    name: 'asOf',
    required: false,
    schema: { type: 'integer', minimum: GENESIS_UNIX_SECONDS, maximum: MAX_ASOF_UNIX_SECONDS },
    description:
      'Unix seconds to value the page at. Defaults to now — amounts accrue every second ' +
      'and the chain emits nothing while they do, so naming the instant is what makes a ' +
      'response reproducible. Echoed back as `valuedAt`.',
  })
  @ApiOkResponse({ type: PositionPageDto })
  @ApiBadRequestResponse({
    type: ApiErrorDto,
    description:
      'A malformed address, a limit outside its bounds, an unrecognised query parameter, ' +
      'or a cursor this service did not issue for this listing.',
  })
  @ApiNotFoundResponse({
    type: ApiErrorDto,
    description:
      'This deployment has never indexed that chain, so it cannot answer for it. Also ' +
      'returned in the window between an indexer starting and recording its first block.',
  })
  async list(
    @Param(new ZodValidationPipe(positionParamsSchema)) params: PositionParams,
    @Query(new ZodValidationPipe(positionQuerySchema)) query: PositionQueryParams,
  ): Promise<PositionPageDto> {
    return this.positions.list(params, query);
  }
}
