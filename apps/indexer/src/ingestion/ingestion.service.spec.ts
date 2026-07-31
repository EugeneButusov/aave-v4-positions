import { Test } from '@nestjs/testing';
import { describe, expect, it, vi } from 'vitest';

import { EVENT_SOURCES, type EventSource } from './event-source';
import { IngestionService } from './ingestion.service';

/** Runs until aborted, then resolves — the shape a real source must honour. */
function blockingSource(name: string): EventSource {
  return {
    name,
    start: (signal) =>
      new Promise<void>((resolve) => {
        signal.addEventListener('abort', () => resolve(), { once: true });
      }),
  };
}

async function buildService(sources?: EventSource[]): Promise<IngestionService> {
  const moduleRef = await Test.createTestingModule({
    providers: [
      IngestionService,
      ...(sources ? [{ provide: EVENT_SOURCES, useValue: sources }] : []),
    ],
  }).compile();

  return moduleRef.get(IngestionService);
}

describe('IngestionService', () => {
  it('starts idle when nothing is registered', async () => {
    const service = await buildService();

    service.onApplicationBootstrap();

    expect(service.sourceNames).toEqual([]);
    await expect(service.onApplicationShutdown()).resolves.toBeUndefined();
  });

  it('starts every registered source', async () => {
    const spoke = {
      name: 'spoke',
      start: vi.fn<EventSource['start']>().mockResolvedValue(undefined),
    };
    const hub = { name: 'hub', start: vi.fn<EventSource['start']>().mockResolvedValue(undefined) };
    const service = await buildService([spoke, hub]);

    service.onApplicationBootstrap();

    expect(spoke.start).toHaveBeenCalledOnce();
    expect(hub.start).toHaveBeenCalledOnce();
    expect(service.sourceNames).toEqual(['spoke', 'hub']);
  });

  it('aborts running sources on shutdown and waits for them to settle', async () => {
    const source = blockingSource('spoke');
    const service = await buildService([source]);

    service.onApplicationBootstrap();
    await service.onApplicationShutdown();

    // Resolving at all proves the abort signal reached the source; without it
    // the shutdown would hang on the still-pending start().
    expect(service.sourceNames).toEqual(['spoke']);
  });

  it('contains a throwing source instead of taking the process down', async () => {
    const failing: EventSource = {
      name: 'broken',
      start: () => Promise.reject(new Error('rpc unreachable')),
    };
    const healthy = blockingSource('spoke');
    const service = await buildService([failing, healthy]);

    service.onApplicationBootstrap();

    await expect(service.onApplicationShutdown()).resolves.toBeUndefined();
  });
});
