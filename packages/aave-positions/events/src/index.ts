export {
  MAIN_SPOKE_ADDRESS,
  MAIN_SPOKE_GENESIS_BLOCK,
  SPOKE_ABI,
  SPOKE_POSITION_EVENTS,
  SPOKE_POSITION_TOPICS,
  isPositionEvent,
  type SpokePositionEvent,
} from './aave/spoke-events';

export type { DecodedEvent } from './decode/decoded-event';
export { SpokeEventDecoder, UndecodableLogError } from './decode/decoder';

export { EVENT_STORE, type EventStore } from './store/event-store';
export { ClickHouseEventStore } from './store/clickhouse-event-store';
export { EVENT_MIGRATIONS_DIR } from './store/clickhouse-event-store';

export { AaveEventProcessor, type AaveEventProcessorOptions } from './aave-event-processor';

export {
  SPOKE_EVENT_PROCESSOR,
  SpokeEventsModule,
  type SpokeEventsAsyncOptions,
  type SpokeEventsOptions,
} from './spoke-events.module';
