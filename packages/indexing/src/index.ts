export {
  CHAIN_CLIENT,
  CHAIN_CLIENT_OPTIONS,
  type Address,
  type BlockHeader,
  type ChainClient,
  type ChainClientOptions,
  type Hash,
  type Hex,
} from './chain/chain-client';
export { ViemChainClient } from './chain/viem-chain-client';
export { ViemLogReader } from './chain/viem-log-reader';
export {
  LOG_READER,
  LogRangeTooLargeError,
  type LogFilter,
  type LogReader,
  type RawLog,
} from './chain/log-reader';
export { ViemErc20MetadataReader } from './chain/viem-erc20-metadata-reader';
export {
  ERC20_METADATA_READER,
  type Erc20MetadataReader,
  type TokenMetadata,
} from './chain/erc20-metadata-reader';

export { CURSOR_STORE, type Cursor, type CursorStore } from './indexing/cursor/cursor-store';
export { PostgresCursorStore } from './indexing/cursor/postgres-cursor-store';
export {
  SYNC_STATUS_STORE,
  type SyncStatus,
  type SyncStatusStore,
} from './indexing/cursor/sync-status-store';
export { PostgresSyncStatusStore } from './indexing/cursor/postgres-sync-status-store';

export {
  BLOCK_PROCESSORS,
  failed,
  ok,
  retry,
  type BlockProcessor,
  type ProcessorOutcome,
} from './indexing/processors/block-processor';
export { LoggingBlockProcessor } from './indexing/processors/logging-block-processor';

export {
  REORG_DETECTOR,
  type ReorgDetector,
  type ReorgVerdict,
} from './indexing/reorg/reorg-detector';
export { HashChainReorgDetector } from './indexing/reorg/hash-chain-reorg-detector';
export { BLOCK_HEADER_STORE, type BlockHeaderStore } from './indexing/reorg/block-header-store';
export { PostgresBlockHeaderStore } from './indexing/reorg/postgres-block-header-store';

export {
  IndexerStatus,
  type IndexerSnapshot,
  type IndexerState,
} from './indexing/observability/indexer-status';
export { IndexerHealthIndicator } from './indexing/observability/indexer.health-indicator';

export {
  BackfillRunner,
  type BackfillRequest,
  type BackfillResult,
} from './backfill/backfill-runner';

export { INDEXING_MIGRATIONS_DIR } from './postgres-migrations';

export { IndexerService, type IterationResult } from './indexing/indexer.service';
export { INDEXING_OPTIONS, type IndexingOptions } from './indexing/indexing.options';
export { IndexingModule, type IndexingAsyncOptions } from './indexing/indexing.module';
