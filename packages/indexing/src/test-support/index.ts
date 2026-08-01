/**
 * Doubles for driving the loop and its seams from a test.
 *
 * Exported from the package rather than kept beside the specs because consumers
 * write processors against `BlockProcessor` and need the same harness — two
 * independent definitions of "a fake that records calls" drift apart.
 *
 * `reorg-harness.ts` sits beside these and is deliberately not re-exported: it
 * builds one concrete detector, so it is a fixture for this package's own specs
 * rather than anything a consumer could hold.
 */
export { FakeChainClient, hashOf, type ChainCall } from './fake-chain-client';
export { ForkingChain } from './forking-chain';
export { RecordingCursorStore } from './recording-cursor-store';
export { RecordingProcessor, type ProcessorCall } from './recording-processor';
export { ScriptedReorgDetector } from './scripted-reorg-detector';
export { ShufflingBlockHeaderStore } from './shuffling-block-header-store';
