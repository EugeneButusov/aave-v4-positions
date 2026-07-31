import { describe, expect, it } from 'vitest';

import { validateEnv } from './env';

describe('validateEnv', () => {
  it('applies defaults for an empty environment', () => {
    const env = validateEnv({});

    expect(env.NODE_ENV).toBe('development');
    expect(env.INDEXER_PORT).toBe(3001);
    expect(env.INDEXER_HOST).toBe('0.0.0.0');
    expect(env.LOG_PRETTY).toBe(false);
  });

  it('coerces numeric variables, which arrive as strings', () => {
    const env = validateEnv({ INDEXER_PORT: '9001', SHUTDOWN_GRACE_SECONDS: '30' });

    expect(env.INDEXER_PORT).toBe(9001);
    expect(env.SHUTDOWN_GRACE_SECONDS).toBe(30);
  });

  it('reads the string forms of a boolean flag', () => {
    expect(validateEnv({ LOG_PRETTY: 'true' }).LOG_PRETTY).toBe(true);
    expect(validateEnv({ LOG_PRETTY: '1' }).LOG_PRETTY).toBe(true);
    expect(validateEnv({ LOG_PRETTY: '0' }).LOG_PRETTY).toBe(false);
  });

  it('rejects an out-of-range port rather than clamping it', () => {
    expect(() => validateEnv({ INDEXER_PORT: '70000' })).toThrow(/INDEXER_PORT/);
  });

  it('rejects an unknown log level', () => {
    expect(() => validateEnv({ LOG_LEVEL: 'verbose' })).toThrow(/LOG_LEVEL/);
  });
});
