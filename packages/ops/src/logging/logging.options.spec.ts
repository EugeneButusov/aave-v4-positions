import type { IncomingMessage, ServerResponse } from 'node:http';

import { describe, expect, it, vi } from 'vitest';

import { REQUEST_ID_HEADER, buildLoggingParams } from './logging.options';

/** Narrow view of the pino-http options this module actually configures. */
interface PinoHttp {
  level?: string;
  transport?: { target: string };
  base?: Record<string, unknown>;
  genReqId: (req: IncomingMessage, res: ServerResponse) => string;
  autoLogging: { ignore: (req: IncomingMessage) => boolean };
  redact: { paths: string[]; remove: boolean };
}

const pinoHttp = (options: Parameters<typeof buildLoggingParams>[0]): PinoHttp =>
  buildLoggingParams(options).pinoHttp as unknown as PinoHttp;

const fakeReq = (url: string, headers: IncomingMessage['headers'] = {}) =>
  ({ url, headers }) as IncomingMessage;

const fakeRes = () => {
  const setHeader = vi.fn<(name: string, value: string) => void>();
  return { res: { setHeader } as unknown as ServerResponse, setHeader };
};

describe('buildLoggingParams', () => {
  it('tags every line with the service name', () => {
    expect(pinoHttp({ service: 'api' }).base).toEqual({ service: 'api' });
  });

  it('merges extra base fields without losing the service tag', () => {
    expect(pinoHttp({ service: 'indexer', base: { chainId: 1 } }).base).toEqual({
      service: 'indexer',
      chainId: 1,
    });
  });

  describe('pretty output', () => {
    it('is off by default, so deployed logs stay one JSON object per line', () => {
      expect(pinoHttp({ service: 'api' }).transport).toBeUndefined();
    });

    it('routes through pino-pretty when asked', () => {
      expect(pinoHttp({ service: 'api', pretty: true }).transport).toMatchObject({
        target: 'pino-pretty',
      });
    });
  });

  describe('request id', () => {
    it('propagates an incoming id rather than minting a new one', () => {
      const { res, setHeader } = fakeRes();
      const id = pinoHttp({ service: 'api' }).genReqId(
        fakeReq('/positions', { [REQUEST_ID_HEADER]: 'from-caller' }),
        res,
      );

      expect(id).toBe('from-caller');
      expect(setHeader).toHaveBeenCalledWith(REQUEST_ID_HEADER, 'from-caller');
    });

    it('mints one when the caller sends none, and echoes it back', () => {
      const { res, setHeader } = fakeRes();
      const id = pinoHttp({ service: 'api' }).genReqId(fakeReq('/positions'), res);

      expect(id).toMatch(/^[0-9a-f-]{36}$/);
      expect(setHeader).toHaveBeenCalledWith(REQUEST_ID_HEADER, id);
    });

    it('takes the first value when a header arrives repeated', () => {
      const { res } = fakeRes();
      const id = pinoHttp({ service: 'api' }).genReqId(
        fakeReq('/positions', { [REQUEST_ID_HEADER]: ['first', 'second'] }),
        res,
      );

      expect(id).toBe('first');
    });
  });

  describe('automatic request logging', () => {
    const ignore = (url: string, prefixes?: string[]) =>
      pinoHttp({
        service: 'api',
        ...(prefixes && { ignorePathPrefixes: prefixes }),
      }).autoLogging.ignore(fakeReq(url));

    it('skips probes, which would otherwise bury everything else', () => {
      expect(ignore('/health/live')).toBe(true);
      expect(ignore('/health/ready')).toBe(true);
    });

    it('logs real traffic', () => {
      expect(ignore('/api/positions/0xabc')).toBe(false);
    });

    it('honours a custom prefix list', () => {
      expect(ignore('/health/live', ['/metrics'])).toBe(false);
      expect(ignore('/metrics', ['/metrics'])).toBe(true);
    });
  });

  it('removes credential-bearing headers rather than masking them', () => {
    const { redact } = pinoHttp({ service: 'api' });

    expect(redact.paths).toEqual(['req.headers.authorization', 'req.headers.cookie']);
    expect(redact.remove).toBe(true);
  });
});
