import { describe, expect, it } from 'vitest';

import { validateEnv } from './env';

/** The one variable with no default; everything else is optional. */
const REQUIRED = { POSITIONS_CURSOR_SECRET: 'a'.repeat(32) };

function env(overrides: Record<string, unknown> = {}): ReturnType<typeof validateEnv> {
  return validateEnv({ ...REQUIRED, ...overrides });
}

describe('validateEnv', () => {
  it('applies defaults once the required variable is present', () => {
    const parsed = env();

    expect(parsed.NODE_ENV).toBe('development');
    expect(parsed.API_PORT).toBe(3000);
    expect(parsed.API_GLOBAL_PREFIX).toBe('api');
    expect(parsed.LOG_PRETTY).toBe(false);
    expect(parsed.API_SYNC_STALE_AFTER_SECONDS).toBe(60);
    expect(parsed.CLICKHOUSE_DATABASE).toBe('default');
  });

  it('coerces numeric variables, which arrive as strings', () => {
    const parsed = env({ API_PORT: '8080', SHUTDOWN_GRACE_SECONDS: '30' });

    expect(parsed.API_PORT).toBe(8080);
    expect(parsed.SHUTDOWN_GRACE_SECONDS).toBe(30);
  });

  it('reads the string forms of a boolean flag', () => {
    expect(env({ LOG_PRETTY: 'true' }).LOG_PRETTY).toBe(true);
    expect(env({ LOG_PRETTY: '1' }).LOG_PRETTY).toBe(true);
    expect(env({ LOG_PRETTY: '0' }).LOG_PRETTY).toBe(false);
  });

  it('rejects an out-of-range port rather than clamping it', () => {
    expect(() => env({ API_PORT: '70000' })).toThrow(/API_PORT/);
  });

  it('rejects an unknown log level', () => {
    expect(() => env({ LOG_LEVEL: 'verbose' })).toThrow(/LOG_LEVEL/);
  });

  it('refuses to start without a cursor secret, rather than inventing one', () => {
    // A default here would be a key every deployment shares, which is not a
    // signature. Failing at boot beats discovering it from forged cursors.
    expect(() => validateEnv({})).toThrow(/POSITIONS_CURSOR_SECRET/);
  });

  it('refuses a cursor secret short enough to guess', () => {
    expect(() => env({ POSITIONS_CURSOR_SECRET: 'too-short' })).toThrow(/POSITIONS_CURSOR_SECRET/);
  });
});
