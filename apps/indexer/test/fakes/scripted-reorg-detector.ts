import type { BlockHeader } from '../../src/chain/chain-client';
import type { Cursor } from '../../src/indexing/cursor-store';
import type { ReorgDetector, ReorgVerdict } from '../../src/indexing/reorg-detector';

const CONTINUOUS: ReorgVerdict = { type: 'continuous' };

/**
 * A detector whose every answer is dictated by the test.
 *
 * The stub that ships never reports a fork, so this is what actually exercises
 * the loop's reorg, bootstrap and finality handling.
 */
export class ScriptedReorgDetector implements ReorgDetector {
  readonly committed: number[] = [];
  readonly rewoundTo: number[] = [];
  readonly bootstrappedWith: (Cursor | null)[] = [];
  readonly inspected: number[] = [];
  /** Full headers, so a test can assert ancestry rather than only block numbers. */
  readonly committedHeaders: BlockHeader[] = [];
  readonly inspectedHeaders: BlockHeader[] = [];

  private bootstrapVerdict: ReorgVerdict = CONTINUOUS;
  private readonly verdicts: ReorgVerdict[] = [];
  /** Nothing is settled by default, so tests take the inspected path unless they opt out. */
  private safeHeadFn: (observedHead: number) => number = () => -1;

  constructor(private readonly trace: string[] = []) {}

  /** Verdicts for successive `inspect` calls; `continuous` once exhausted. */
  queue(...verdicts: ReorgVerdict[]): this {
    this.verdicts.push(...verdicts);
    return this;
  }

  onBootstrap(verdict: ReorgVerdict): this {
    this.bootstrapVerdict = verdict;
    return this;
  }

  /** Treat everything up to and including `blockNumber` as settled. */
  settledThrough(blockNumber: number): this {
    this.safeHeadFn = () => blockNumber;
    return this;
  }

  /** Settle everything more than `depth` blocks below the head. */
  settledBelowDepth(depth: number): this {
    this.safeHeadFn = (observedHead) => Math.max(-1, observedHead - depth);
    return this;
  }

  bootstrap(cursor: Cursor | null): Promise<ReorgVerdict> {
    this.trace.push('detector.bootstrap');
    this.bootstrappedWith.push(cursor);
    return Promise.resolve(this.bootstrapVerdict);
  }

  safeHead(observedHead: number): number {
    return this.safeHeadFn(observedHead);
  }

  inspect(header: BlockHeader): ReorgVerdict {
    this.trace.push(`detector.inspect(${header.number})`);
    this.inspected.push(header.number);
    this.inspectedHeaders.push(header);
    return this.verdicts.shift() ?? CONTINUOUS;
  }

  commit(header: BlockHeader): void {
    this.trace.push(`detector.commit(${header.number})`);
    this.committed.push(header.number);
    this.committedHeaders.push(header);
  }

  rewindTo(lastValidBlock: number): void {
    this.trace.push(`detector.rewindTo(${lastValidBlock})`);
    this.rewoundTo.push(lastValidBlock);
  }
}
