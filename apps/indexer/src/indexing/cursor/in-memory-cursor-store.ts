import { Injectable } from '@nestjs/common';

import type { Cursor, CursorStore } from './cursor-store';

/**
 * Keeps the cursor in process memory.
 *
 * This is the whole of the persistence layer for now, and it does not survive a
 * restart — so the resume path is exercised by tests through the port rather
 * than by the running service. The database adapter replaces this class and
 * nothing else; see {@link CursorStore} for the transaction seam it will need.
 */
@Injectable()
export class InMemoryCursorStore implements CursorStore {
  private readonly cursors = new Map<number, Cursor>();

  load(chainId: number): Promise<Cursor | null> {
    return Promise.resolve(this.cursors.get(chainId) ?? null);
  }

  save(cursor: Cursor): Promise<void> {
    this.cursors.set(cursor.chainId, cursor);
    return Promise.resolve();
  }
}
