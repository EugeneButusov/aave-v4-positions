/**
 * Doubles for driving the loop and its seams from a test, and the contract
 * suites every adapter of a persistence port must pass.
 *
 * Exported from the package rather than kept beside the specs because consumers
 * write processors against `BlockProcessor` and need the same harness — two
 * independent definitions of "a fake that records calls" drift apart. The
 * contracts are here for a stronger version of the same reason: a store adapter
 * lives in whichever package owns its tables, so the semantics it has to satisfy
 * cannot live beside the one implementation that happens to be nearest.
 *
 * `reorg-harness.ts` sits beside these and is deliberately not re-exported: it
 * builds one concrete detector, so it is a fixture for this package's own specs
 * rather than anything a consumer could hold.
 */
export {
  describeBlockHeaderStoreContract,
  type BlockHeaderStoreContract,
} from './block-header-store-contract';
export { describeCursorStoreContract, type CursorStoreContract } from './cursor-store-contract';
export { FakeChainClient, hashOf, type ChainCall } from './fake-chain-client';
export { ForkingChain } from './forking-chain';
export { RecordingBlockHeaderStore } from './recording-block-header-store';
export { RecordingCursorStore } from './recording-cursor-store';
export { RecordingProcessor, type ProcessorCall } from './recording-processor';
export { ScriptedReorgDetector } from './scripted-reorg-detector';
export { ShufflingBlockHeaderStore } from './shuffling-block-header-store';
