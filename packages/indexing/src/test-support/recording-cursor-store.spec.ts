import { describeCursorStoreContract } from './cursor-store-contract';
import { RecordingCursorStore } from './recording-cursor-store';

// The fake runs the port's contract too, not just the real adapter. The loop's
// own specs prove "the cursor is the commit point" through this class, so a fake
// that quietly disagreed with the port would turn each of those into a proof
// about a fiction — and it is the only cursor store left that answers without a
// database, so nothing else would catch the drift.
describeCursorStoreContract('RecordingCursorStore', {
  fresh: () => Promise.resolve(new RecordingCursorStore()),
});
