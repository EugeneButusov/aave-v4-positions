import { describe, expect, it } from 'vitest';

import { validateEnv } from './env';

/** The two variables with no default; everything else is optional. */
const REQUIRED = { CHAIN_ID: '1', RPC_URLS: 'https://rpc.example.com' };

function env(overrides: Record<string, unknown> = {}): ReturnType<typeof validateEnv> {
  return validateEnv({ ...REQUIRED, ...overrides });
}

describe('validateEnv', () => {
  it('applies defaults once the required variables are present', () => {
    const parsed = env();

    expect(parsed.NODE_ENV).toBe('development');
    expect(parsed.INDEXER_PORT).toBe(3001);
    expect(parsed.INDEXER_HOST).toBe('0.0.0.0');
    expect(parsed.LOG_PRETTY).toBe(false);
    expect(parsed.FINALITY_DEPTH).toBe(128);
    // Core Hub genesis, not the Main Spoke's: the floor is the earliest of
    // the contracts followed, or the earlier one starts mid-history.
    expect(parsed.INDEXER_START_BLOCK).toBe(24_720_891);
    expect(parsed.INDEXER_MAX_RANGE_SIZE).toBe(1_000);
    expect(parsed.INDEXER_AUTOSTART).toBe(true);
  });

  it('coerces numeric variables, which arrive as strings', () => {
    const parsed = env({ INDEXER_PORT: '9001', SHUTDOWN_GRACE_SECONDS: '30', CHAIN_ID: '137' });

    expect(parsed.INDEXER_PORT).toBe(9001);
    expect(parsed.SHUTDOWN_GRACE_SECONDS).toBe(30);
    expect(parsed.CHAIN_ID).toBe(137);
  });

  it('reads the string forms of a boolean flag', () => {
    expect(env({ LOG_PRETTY: 'true' }).LOG_PRETTY).toBe(true);
    expect(env({ LOG_PRETTY: '1' }).LOG_PRETTY).toBe(true);
    expect(env({ LOG_PRETTY: '0' }).LOG_PRETTY).toBe(false);
    expect(env({ INDEXER_AUTOSTART: 'false' }).INDEXER_AUTOSTART).toBe(false);
  });

  it('splits and trims a provider list', () => {
    const parsed = env({ RPC_URLS: ' https://a.example.com , https://b.example.com ' });

    expect(parsed.RPC_URLS).toEqual(['https://a.example.com', 'https://b.example.com']);
  });

  it('tolerates a trailing separator in the provider list', () => {
    expect(env({ RPC_URLS: 'https://a.example.com,' }).RPC_URLS).toEqual(['https://a.example.com']);
  });

  it('rejects an out-of-range port rather than clamping it', () => {
    expect(() => env({ INDEXER_PORT: '70000' })).toThrow(/INDEXER_PORT/);
  });

  it('rejects an unknown log level', () => {
    expect(() => env({ LOG_LEVEL: 'verbose' })).toThrow(/LOG_LEVEL/);
  });

  it('refuses to start without a chain id', () => {
    // No default: an indexer pointed at the wrong chain produces plausible,
    // wrong data rather than an error, so the value must be stated.
    expect(() => validateEnv({ RPC_URLS: 'https://a.example.com' })).toThrow(/CHAIN_ID/);
  });

  it('refuses to start without a provider', () => {
    expect(() => validateEnv({ CHAIN_ID: '1' })).toThrow(/RPC_URLS/);
  });

  it('rejects an empty provider list', () => {
    expect(() => env({ RPC_URLS: '  ,  ' })).toThrow(/RPC_URLS/);
  });

  it('rejects a provider that is not a URL', () => {
    expect(() => env({ RPC_URLS: 'https://a.example.com,not-a-url' })).toThrow(/RPC_URLS/);
  });

  it('rejects a negative finality depth', () => {
    expect(() => env({ FINALITY_DEPTH: '-1' })).toThrow(/FINALITY_DEPTH/);
  });

  it('rejects a zero-width range size', () => {
    expect(() => env({ INDEXER_MAX_RANGE_SIZE: '0' })).toThrow(/INDEXER_MAX_RANGE_SIZE/);
  });
});
