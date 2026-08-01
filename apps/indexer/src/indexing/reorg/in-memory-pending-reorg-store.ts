import { Injectable } from '@nestjs/common';

import type { PendingReorg, PendingReorgStore } from './pending-reorg-store';

/**
 * Keeps the owed reorg in process memory.
 *
 * Which is to say it does not survive the crash it exists to survive — the
 * whole point of the port is a durable adapter, and this is the placeholder
 * that lets the protocol be built and tested ahead of one. The replay path is
 * exercised through the port rather than by the running service.
 */
@Injectable()
export class InMemoryPendingReorgStore implements PendingReorgStore {
  private readonly pending = new Map<number, PendingReorg>();

  load(chainId: number): Promise<PendingReorg | null> {
    return Promise.resolve(this.pending.get(chainId) ?? null);
  }

  save(chainId: number, reorg: PendingReorg): Promise<void> {
    this.pending.set(chainId, reorg);
    return Promise.resolve();
  }

  clear(chainId: number): Promise<void> {
    this.pending.delete(chainId);
    return Promise.resolve();
  }
}
