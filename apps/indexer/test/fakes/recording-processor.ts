import { ok, type BlockProcessor, type ProcessorOutcome } from '../../src/indexing/block-processor';

export type ProcessorCall =
  | {
      readonly kind: 'range';
      readonly from: number;
      readonly to: number;
      readonly aborted: boolean;
    }
  | {
      readonly kind: 'reorg';
      readonly firstInvalidBlock: number;
      readonly lastInvalidBlock: number;
      readonly aborted: boolean;
    };

/**
 * Records what it was asked to do and returns scripted outcomes, defaulting to
 * `ok` once the queue is exhausted.
 *
 * `trace` is shared with {@link ScriptedReorgDetector} so a test can assert the
 * *relative* order of detector and processor calls — which is the only way to
 * prove the reorg check happens before dispatch rather than alongside it.
 */
export class RecordingProcessor implements BlockProcessor {
  readonly calls: ProcessorCall[] = [];

  private readonly outcomes: ProcessorOutcome[] = [];
  private thrown: Error | null = null;

  constructor(
    readonly name = 'recorder',
    private readonly trace: string[] = [],
  ) {}

  /** Outcomes returned by successive calls, in order. */
  queue(...outcomes: ProcessorOutcome[]): this {
    this.outcomes.push(...outcomes);
    return this;
  }

  /** Throw instead of returning, on every call, until cleared. */
  throwOnCall(error: Error): this {
    this.thrown = error;
    return this;
  }

  onBlockRange(from: number, to: number, signal: AbortSignal): ProcessorOutcome {
    this.trace.push(`${this.name}.onBlockRange(${from},${to})`);
    this.calls.push({ kind: 'range', from, to, aborted: signal.aborted });
    return this.next();
  }

  onReorg(
    firstInvalidBlock: number,
    lastInvalidBlock: number,
    signal: AbortSignal,
  ): ProcessorOutcome {
    this.trace.push(`${this.name}.onReorg(${firstInvalidBlock},${lastInvalidBlock})`);
    this.calls.push({
      kind: 'reorg',
      firstInvalidBlock,
      lastInvalidBlock,
      aborted: signal.aborted,
    });
    return this.next();
  }

  private next(): ProcessorOutcome {
    if (this.thrown) throw this.thrown;
    return this.outcomes.shift() ?? ok();
  }
}
