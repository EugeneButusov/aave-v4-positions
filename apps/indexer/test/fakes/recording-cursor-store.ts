import type { Cursor, CursorStore } from '../../src/indexing/cursor/cursor-store';

/**
 * An in-memory cursor store that records every write and can be made to fail on
 * a chosen save — which is how the "cursor is the commit point" property is
 * proven: if the save throws, the same range must be dispatched again.
 */
export class RecordingCursorStore implements CursorStore {
  readonly saved: Cursor[] = [];

  private readonly cursors = new Map<number, Cursor>();
  private failOnSave: number | null = null;
  private saveAttempts = 0;

  constructor(initial?: Cursor) {
    if (initial) this.cursors.set(initial.chainId, initial);
  }

  /** Make the `nth` save (1-based) reject. */
  failSave(nth: number): this {
    this.failOnSave = nth;
    return this;
  }

  load(chainId: number): Promise<Cursor | null> {
    return Promise.resolve(this.cursors.get(chainId) ?? null);
  }

  save(cursor: Cursor): Promise<void> {
    this.saveAttempts += 1;
    if (this.saveAttempts === this.failOnSave) {
      return Promise.reject(new Error('cursor store unavailable'));
    }

    this.cursors.set(cursor.chainId, cursor);
    this.saved.push(cursor);
    return Promise.resolve();
  }
}
