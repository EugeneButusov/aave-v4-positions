export {
  CLICKHOUSE_CLIENT,
  CLICKHOUSE_OPTIONS,
  type ClickHouseOptions,
} from './clickhouse.options';
export { ClickHouseModule, type ClickHouseAsyncOptions } from './clickhouse.module';
export { ClickHouseHealthIndicator } from './clickhouse.health-indicator';
// The migration helpers are not re-exported. They moved to `@packages/migrations`
// when a second database arrived, and one symbol wants one import path —
// re-exporting them from here would leave two, and the wrong one is the one that
// suggests loading migrations is a ClickHouse concern.
export { migrate } from './migrate';
