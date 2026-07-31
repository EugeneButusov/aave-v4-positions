import { describe, expect, it } from 'vitest';

import { validateEnv } from './env';

const minimal = { RPC_URL: 'https://eth.drpc.org' };

describe('validateEnv', () => {
  it('applies defaults around the required RPC endpoint', () => {
    const env = validateEnv(minimal);

    expect(env.NODE_ENV).toBe('development');
    expect(env.INDEXER_PORT).toBe(3001);
    expect(env.CHAIN_ID).toBe(1);
    expect(env.LOG_PRETTY).toBe(false);
  });

  it('refuses to boot without an RPC endpoint', () => {
    expect(() => validateEnv({})).toThrow(/RPC_URL/);
  });

  it('rejects a malformed RPC endpoint', () => {
    expect(() => validateEnv({ RPC_URL: 'not-a-url' })).toThrow(/RPC_URL/);
    expect(() => validateEnv({ RPC_URL: 'ftp://eth.example' })).toThrow(/RPC_URL/);
  });

  it('accepts websocket endpoints', () => {
    expect(validateEnv({ RPC_URL: 'wss://eth.example/ws' }).RPC_URL).toBe('wss://eth.example/ws');
  });

  it('coerces numeric variables, which arrive as strings', () => {
    const env = validateEnv({ ...minimal, CHAIN_ID: '8453', INDEXER_PORT: '9001' });

    expect(env.CHAIN_ID).toBe(8453);
    expect(env.INDEXER_PORT).toBe(9001);
  });
});
