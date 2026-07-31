import { Controller, Get } from '@nestjs/common';
import { ApiOkResponse, ApiOperation, ApiTags } from '@nestjs/swagger';

import { HelloResponseDto } from './hello.dto';

/**
 * Placeholder so the API has a business surface under the global prefix while
 * the positions endpoints are built. Replaced, not extended, by the real ones.
 */
@ApiTags('hello')
@Controller('hello')
export class HelloController {
  @Get()
  @ApiOperation({ summary: 'Stub endpoint' })
  @ApiOkResponse({ type: HelloResponseDto })
  hello(): HelloResponseDto {
    return { message: 'aave-v4-positions api' };
  }
}
