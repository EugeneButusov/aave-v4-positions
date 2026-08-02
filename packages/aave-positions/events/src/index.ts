export {
  MAIN_SPOKE_ADDRESS,
  MAIN_SPOKE_GENESIS_BLOCK,
  SPOKE_ABI,
  SPOKE_POSITION_EVENTS,
  SPOKE_POSITION_TOPICS,
  isPositionEvent,
  type SpokePositionEvent,
} from './aave/spoke-events';

export {
  CORE_HUB_ADDRESS,
  CORE_HUB_GENESIS_BLOCK,
  HUB_ABI,
  HUB_STATE_EVENTS,
  HUB_STATE_TOPICS,
  isHubStateEvent,
  type HubStateEvent,
} from './aave/hub-events';

export type { DecodedEvent } from './decode/decoded-event';
export { ContractLogDecoder, UndecodableLogError } from './decode/decoder';
export { SpokeEventDecoder } from './decode/spoke-event-decoder';
export { HubEventDecoder } from './decode/hub-event-decoder';

export { HUB_EVENT_STORE, SPOKE_EVENT_STORE, type EventStore } from './store/event-store';
export { ClickHouseEventLedger, EVENT_MIGRATIONS_DIR } from './store/clickhouse-event-store';
export {
  ClickHouseSpokeEventStore,
  SPOKE_EVENTS_TABLE,
  SPOKE_EVENTS_VIEW,
} from './store/clickhouse-spoke-event-store';
export {
  ClickHouseHubEventStore,
  HUB_EVENTS_TABLE,
  HUB_EVENTS_VIEW,
} from './store/clickhouse-hub-event-store';

export {
  AaveEventProcessor,
  hubEventSource,
  spokeEventSource,
  type EventSource,
} from './aave-event-processor';

export {
  SPOKE_EVENT_PROCESSOR,
  SpokeEventsModule,
  type SpokeEventsAsyncOptions,
  type SpokeEventsOptions,
} from './spoke-events.module';

export {
  HUB_EVENT_PROCESSOR,
  HubEventsModule,
  type HubEventsAsyncOptions,
  type HubEventsOptions,
} from './hub-events.module';
