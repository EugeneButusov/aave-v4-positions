import { Injectable } from '@nestjs/common';

import { ClickHouseEventLedger } from './clickhouse-event-store';

export const HUB_EVENTS_TABLE = 'hub_events';
export const HUB_EVENTS_VIEW = 'hub_events_current';

/** The Hub's asset-state events. `topic1` is an asset id. */
@Injectable()
export class ClickHouseHubEventStore extends ClickHouseEventLedger {
  protected readonly table = HUB_EVENTS_TABLE;
  protected readonly view = HUB_EVENTS_VIEW;
}
