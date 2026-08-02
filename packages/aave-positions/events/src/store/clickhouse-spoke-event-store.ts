import { Injectable } from '@nestjs/common';

import { ClickHouseEventLedger } from './clickhouse-event-store';

export const SPOKE_EVENTS_TABLE = 'spoke_events';
export const SPOKE_EVENTS_VIEW = 'spoke_events_current';

/** The Spoke's position events. `topic1` is a reserve id. */
@Injectable()
export class ClickHouseSpokeEventStore extends ClickHouseEventLedger {
  protected readonly table = SPOKE_EVENTS_TABLE;
  protected readonly view = SPOKE_EVENTS_VIEW;
}
