import { describe, expect, it } from 'vitest';

import { LoggingBlockProcessor } from './logging-block-processor';

describe('LoggingBlockProcessor', () => {
  it('succeeds on a dispatched range', () => {
    expect(new LoggingBlockProcessor().onBlockRange(100, 199)).toEqual({ status: 'ok' });
  });

  it('succeeds on a reorg', () => {
    expect(new LoggingBlockProcessor().onReorg(150, 160)).toEqual({ status: 'ok' });
  });

  it('is named, so an outcome can be attributed to it', () => {
    expect(new LoggingBlockProcessor().name).toBe('logging');
  });
});
