import { describe, expect, it } from 'vitest';

import { RecordingProcessor } from '../../test-support/recording-processor';
import { failed, retry, type BlockProcessor } from './block-processor';
import { dispatchToProcessors } from './dispatch';

const NEVER_ABORTS = new AbortController().signal;

/** Every test dispatches the same range; only the outcomes differ. */
function dispatch(processors: readonly BlockProcessor[]): ReturnType<typeof dispatchToProcessors> {
  return dispatchToProcessors(processors, NEVER_ABORTS, (processor, signal) =>
    processor.onBlockRange(1, 10, signal),
  );
}

describe('dispatchToProcessors', () => {
  it('accepts an empty list, so an unconfigured loop is not an error', async () => {
    expect(await dispatch([])).toEqual({ status: 'ok' });
  });

  it('invokes every processor in registration order', async () => {
    const trace: string[] = [];
    const processors = [
      new RecordingProcessor('first', trace),
      new RecordingProcessor('second', trace),
    ];

    expect(await dispatch(processors)).toEqual({ status: 'ok' });
    expect(trace).toEqual(['first.onBlockRange(1,10)', 'second.onBlockRange(1,10)']);
  });

  it('stops at the first processor that does not accept', async () => {
    const trace: string[] = [];
    const processors = [
      new RecordingProcessor('first', trace),
      new RecordingProcessor('second', trace).queue(retry('busy')),
      new RecordingProcessor('third', trace),
    ];

    const result = await dispatch(processors);

    expect(result.status).toBe('retry');
    expect(trace).toEqual(['first.onBlockRange(1,10)', 'second.onBlockRange(1,10)']);
  });

  it('names the processor that decided the outcome, and composes its reason', async () => {
    const result = await dispatch([new RecordingProcessor('decoder').queue(retry('rate limited'))]);

    expect(result).toEqual({
      status: 'retry',
      processor: 'decoder',
      reason: 'decoder: rate limited',
      narrowRange: false,
    });
  });

  it('normalises an absent narrowRange to false rather than leaving it undefined', async () => {
    const result = await dispatch([new RecordingProcessor('p').queue(retry('busy'))]);

    expect(result).toMatchObject({ narrowRange: false });
  });

  it('carries narrowRange through when a processor asks for it', async () => {
    const outcome = retry('range too wide', { narrowRange: true });
    const result = await dispatch([new RecordingProcessor('p').queue(outcome)]);

    expect(result).toMatchObject({ narrowRange: true });
  });

  it('treats a thrown error as a retry, not a failure', async () => {
    const processor = new RecordingProcessor('flaky').throwOnCall(new Error('socket hang up'));

    expect(await dispatch([processor])).toEqual({
      status: 'retry',
      processor: 'flaky',
      reason: 'flaky threw: socket hang up',
      narrowRange: false,
    });
  });

  it('reports a failed outcome as failed, so the caller can stop rather than retry', async () => {
    const processor = new RecordingProcessor('decoder').queue(failed('unknown event schema'));

    expect(await dispatch([processor])).toEqual({
      status: 'failed',
      processor: 'decoder',
      reason: 'decoder: unknown event schema',
    });
  });
});
