import { describeCursorStoreContract } from '../../test-support/cursor-store-contract';
import { InMemoryCursorStore } from './in-memory-cursor-store';

// The whole of this adapter's behaviour is the port's behaviour, so the shared
// contract is the whole spec. Anything it could assert on top would be about the
// Map, which is not a promise this class makes.
describeCursorStoreContract('InMemoryCursorStore', {
  fresh: () => Promise.resolve(new InMemoryCursorStore()),
});
