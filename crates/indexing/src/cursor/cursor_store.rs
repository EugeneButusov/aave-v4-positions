use alloy_primitives::B256;
use async_trait::async_trait;
use units::ChainId;

use crate::cursor::Error;

/// How far indexing has got on one chain, and on which branch.
///
/// `last_hash` is never absent, including below the finality boundary where no
/// fork can reach it: a resume re-anchors from the cursor alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub chain_id: ChainId,
    pub last_block: u64,
    pub last_hash: B256,
}

/// The pipeline's single durable commit point.
///
/// No transaction seam, and none is possible: processors write to a different
/// database. Dispatch is at-least-once and processors are idempotent instead.
#[async_trait]
pub trait CursorStore: Send + Sync {
    /// `None` when this chain has never been indexed, which is what sends the
    /// loop to its configured start block.
    ///
    /// # Errors
    ///
    /// [`Error`] when the row cannot be read.
    async fn load(&self, chain_id: ChainId) -> Result<Option<Cursor>, Error>;

    /// **Durable by the time this resolves.** A write-behind adapter can leave
    /// the cursor below the window, and a resume point nothing can vet turns
    /// every cross-restart fork into an unrecoverable one.
    ///
    /// **A cursor may move backwards, and this must accept it.** Unwinding a
    /// fork saves the last *valid* block, below where the cursor stood, so the
    /// monotonic guard a reviewer reaches for —
    /// `WHERE last_block < EXCLUDED.last_block` — would report success and
    /// discard the rewind.
    ///
    /// # Errors
    ///
    /// [`Error`] when the row cannot be written.
    async fn save(&self, cursor: &Cursor) -> Result<(), Error>;
}
