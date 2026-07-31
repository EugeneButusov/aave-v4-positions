import { describe, expect, it } from 'vitest';

import { validateEnv } from './env';

describe('validateEnv', () => {
  it('applies defaults for an empty environment', () => {
    const env = validateEnv({});

    expect(env.NODE_ENV).toBe('development');
    expect(env.API_PORT).toBe(3000);
    expect(env.API_GLOBAL_PREFIX).toBe('api');
    expect(env.LOG_PRETTY).toBe(false);
  });

  it('coerces numeric variables, which arrive as strings', () => {
    const env = validateEnv({ API_PORT: '8080', SHUTDOWN_GRACE_SECONDS: '30' });

    expect(env.API_PORT).toBe(8080);
    expect(env.SHUTDOWN_GRACE_SECONDS).toBe(30);
  });

  it('reads the string forms of a boolean flag', () => {
    expect(validateEnv({ LOG_PRETTY: 'true' }).LOG_PRETTY).toBe(true);
    expect(validateEnv({ LOG_PRETTY: '1' }).LOG_PRETTY).toBe(true);
    expect(validateEnv({ LOG_PRETTY: '0' }).LOG_PRETTY).toBe(false);
  });

  it('rejects an out-of-range port rather than clamping it', () => {
    expect(() => validateEnv({ API_PORT: '70000' })).toThrow(/API_PORT/);
  });

  it('rejects an unknown log level', () => {
    expect(() => validateEnv({ LOG_LEVEL: 'verbose' })).toThrow(/LOG_LEVEL/);
  });
});
