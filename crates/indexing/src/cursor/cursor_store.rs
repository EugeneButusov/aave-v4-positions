use alloy_primitives::B256;
use async_trait::async_trait;

use crate::cursor::Error;

/// How far indexing has got on one chain, and on which branch.
///
/// `last_hash` has exactly one meaning and is never absent: the hash of
/// `last_block` as last seen. The loop reads it at the top of every dispatched
/// range, including ranges below the finality boundary where it is not needed
/// for correctness, so the invariant holds unconditionally and a resume can
/// re-anchor from the cursor alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub chain_id: u32,
    pub last_block: u64,
    pub last_hash: B256,
}

/// The pipeline's single durable commit point.
///
/// **There is no transaction seam here, and there will not be one.** Processors
/// write to a different database, so no cursor store could commit atomically
/// with them. Dispatch is at-least-once and processors are idempotent instead,
/// which is the whole answer rather than a stopgap.
#[async_trait]
pub trait CursorStore: Send + Sync {
    /// `None` when this chain has never been indexed, which is what sends the
    /// loop to its configured start block.
    ///
    /// # Errors
    ///
    /// [`Error`] when the row cannot be read.
    async fn load(&self, chain_id: u32) -> Result<Option<Cursor>, Error>;

    /// **Durable by the time this resolves.** An adapter that buffers breaks
    /// the only ordering the pipeline has: the header is committed before this
    /// is saved and a rewind happens after, so a crash leaves the window at or
    /// ahead of the cursor. A write-behind adapter can leave it behind, and a
    /// resume point nothing can vet turns every cross-restart fork into an
    /// unrecoverable one.
    ///
    /// **A cursor may move backwards, and this must accept it.** Unwinding a
    /// fork saves the last *valid* block, which is below where the cursor
    /// already stood. The monotonic guard a reviewer reaches for —
    /// `WHERE last_block < EXCLUDED.last_block` — would report success,
    /// discard the rewind, and leave the loop continuing from a branch that
    /// lost.
    ///
    /// # Errors
    ///
    /// [`Error`] when the row cannot be written. The caller then re-dispatches
    /// the same range rather than advancing.
    async fn save(&self, cursor: &Cursor) -> Result<(), Error>;
}
