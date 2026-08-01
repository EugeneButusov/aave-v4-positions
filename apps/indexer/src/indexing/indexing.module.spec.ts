import { Inject, Injectable } from '@nestjs/common';
import { Test } from '@nestjs/testing';
import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  CHAIN_CLIENT,
  CHAIN_CLIENT_OPTIONS,
  type ChainClient,
  type ChainClientOptions,
} from '../chain/chain-client';
import { ViemChainClient } from '../chain/viem-chain-client';
import {
  BLOCK_PROCESSORS,
  ok,
  type BlockProcessor,
  type ProcessorOutcome,
} from './block-processor';
import { CURSOR_STORE, type CursorStore } from './cursor-store';
import { InMemoryCursorStore } from './in-memory-cursor-store';
import { IndexerHealthIndicator } from './indexer.health-indicator';
import { IndexerService } from './indexer.service';
import { INDEXING_OPTIONS, type IndexingOptions } from './indexing.options';
import { IndexingModule } from './indexing.module';
import { NoopReorgDetector } from './noop-reorg-detector';

const SETTINGS = Symbol('SETTINGS');

interface Settings {
  readonly chainId: number;
  readonly rpcUrls: string[];
}

function optionsFrom(settings: Settings): IndexingOptions {
  return {
    chainId: settings.chainId,
    rpcUrls: settings.rpcUrls,
    rpcTimeoutMs: 1_000,
    finalityDepth: 128,
    startBlock: 100,
    maxRangeSize: 50,
    pollIntervalMs: 1_000,
    stallThresholdMs: 60_000,
    autoStart: false,
  };
}

@Injectable()
class Marker {
  readonly tag = 'injected';
}

/** Proves a processor is resolved through DI rather than instantiated bare. */
@Injectable()
class DependentProcessor implements BlockProcessor {
  readonly name: string;

  constructor(marker: Marker) {
    this.name = marker.tag;
  }

  onBlockRange(): ProcessorOutcome {
    return ok();
  }

  onReorg(): ProcessorOutcome {
    return ok();
  }
}

@Injectable()
class SecondProcessor implements BlockProcessor {
  readonly name = 'second';

  constructor(@Inject(CURSOR_STORE) readonly store: CursorStore) {}

  onBlockRange(): ProcessorOutcome {
    return ok();
  }

  onReorg(): ProcessorOutcome {
    return ok();
  }
}

function build(settings: Settings = { chainId: 1, rpcUrls: ['https://rpc.example.com'] }) {
  return Test.createTestingModule({
    imports: [
      IndexingModule.forRootAsync({
        providers: [
          { provide: SETTINGS, useValue: settings },
          Marker,
          DependentProcessor,
          SecondProcessor,
          NoopReorgDetector,
          InMemoryCursorStore,
        ],
        inject: [SETTINGS],
        useFactory: optionsFrom,
        processors: [DependentProcessor, SecondProcessor],
        reorgDetector: NoopReorgDetector,
        cursorStore: InMemoryCursorStore,
      }),
    ],
  }).compile();
}

describe('IndexingModule', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('resolves its options from an injected dependency', async () => {
    const moduleRef = await build({ chainId: 137, rpcUrls: ['https://polygon.example.com'] });

    expect(moduleRef.get<IndexingOptions>(INDEXING_OPTIONS).chainId).toBe(137);
  });

  it('collects processors in registration order', async () => {
    const moduleRef = await build();

    const processors = moduleRef.get<BlockProcessor[]>(BLOCK_PROCESSORS);

    expect(processors.map((p) => p.name)).toEqual(['injected', 'second']);
  });

  it('resolves each processor through DI, with its own dependencies', async () => {
    const moduleRef = await build();

    const processors = moduleRef.get<BlockProcessor[]>(BLOCK_PROCESSORS);

    // 'injected' comes from Marker, so the processor was constructed by the
    // injector rather than handed over as a bare value.
    expect(processors[0]?.name).toBe('injected');
    expect((processors[1] as SecondProcessor).store).toBeInstanceOf(InMemoryCursorStore);
  });

  it('binds the chain client over the configured providers', async () => {
    const moduleRef = await build({
      chainId: 1,
      rpcUrls: ['https://a.example.com', 'https://b.example.com'],
    });

    expect(moduleRef.get<ChainClient>(CHAIN_CLIENT)).toBeInstanceOf(ViemChainClient);
    // The chain layer sees the provider list under its own narrow token, not
    // the whole indexing options type.
    expect(moduleRef.get<ChainClientOptions>(CHAIN_CLIENT_OPTIONS).rpcUrls).toEqual([
      'https://a.example.com',
      'https://b.example.com',
    ]);
  });

  it('reaches no network while building the module', async () => {
    const fetch = vi.fn<() => Promise<Response>>();
    vi.stubGlobal('fetch', fetch);

    const moduleRef = await build();
    moduleRef.get<ChainClient>(CHAIN_CLIENT);

    // Boot must not depend on a reachable node — otherwise a provider outage
    // turns into a crash loop instead of a pod reporting not-ready.
    expect(fetch).not.toHaveBeenCalled();
  });

  it('exposes the loop and its health indicator to the importing module', async () => {
    const moduleRef = await build();

    expect(moduleRef.get(IndexerService)).toBeInstanceOf(IndexerService);
    expect(moduleRef.get(IndexerHealthIndicator)).toBeInstanceOf(IndexerHealthIndicator);
  });

  it('produces a distinct module on every call, which is why callers must reuse one', async () => {
    const first = IndexingModule.forRootAsync({
      providers: [NoopReorgDetector, InMemoryCursorStore],
      useFactory: () => optionsFrom({ chainId: 1, rpcUrls: ['https://a.example.com'] }),
      reorgDetector: NoopReorgDetector,
      cursorStore: InMemoryCursorStore,
    });
    const second = IndexingModule.forRootAsync({
      providers: [NoopReorgDetector, InMemoryCursorStore],
      useFactory: () => optionsFrom({ chainId: 1, rpcUrls: ['https://a.example.com'] }),
      reorgDetector: NoopReorgDetector,
      cursorStore: InMemoryCursorStore,
    });

    // Nest keys a dynamic module by object reference. Importing both would give
    // two IndexerService instances and two loops racing the same cursor, which
    // is why AppModule builds this once into a const.
    expect(first).not.toBe(second);

    const moduleRef = await Test.createTestingModule({ imports: [first, second] }).compile();
    expect(moduleRef.get(IndexerService)).toBeInstanceOf(IndexerService);
  });
});
