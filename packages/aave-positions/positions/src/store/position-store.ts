import type { Address } from '@packages/indexing';

import type { Position } from './position';

export interface PositionQuery {
  readonly chainId: number;
  /** Lower-cased by the store, so a checksummed address from a caller still matches. */
  readonly user?: Address;
  readonly spoke?: Address;
  readonly limit: number;
  /** Opaque; hand back {@link PositionPage.nextCursor} verbatim. */
  readonly cursor?: string;
}

export interface PositionPage {
  readonly items: readonly Position[];
  /** `null` on the last page. Absence is the end, not an empty string. */
  readonly nextCursor: string | null;
}

/**
 * Reads the folded positions.
 *
 * Read-only by construction: nothing in this package writes. The fold is
 * materialized views over the event ledger, so positions advance because the
 * indexer appended events — there is no ingestion path here to keep in step
 * with one, and a reorg repairs the projection without this store learning of
 * it.
 *
 * **Only open positions.** §12.1: a position exists while its shares are
 * non-zero. A closed one keeps its row and its event count, and is filtered out
 * here rather than deleted anywhere.
 */
export interface PositionStore {
  list(query: PositionQuery): Promise<PositionPage>;
}

export const POSITION_STORE = Symbol('POSITION_STORE');
