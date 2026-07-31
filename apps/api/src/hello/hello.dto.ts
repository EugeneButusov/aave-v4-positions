import { ApiProperty } from '@nestjs/swagger';

export class HelloResponseDto {
  @ApiProperty({ example: 'aave-v4-positions api' })
  message!: string;
}
