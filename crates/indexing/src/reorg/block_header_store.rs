use async_trait::async_trait;

use crate::chain::BlockHeader;
use crate::reorg::Error;

/// The retention window: one contiguous run of headers per chain, the deepest
/// recoverable fork's worth.
///
/// **Durable, and there is only one adapter.** An in-memory window under a
/// durable cursor is strictly worse than neither — the resume point is exactly
/// what a fork reorged out, so a bootstrap would hold one hash, find nothing
/// retained, and answer unrecoverable for every fork across a restart.
#[async_trait]
pub trait BlockHeaderStore: Send + Sync {
    /// Everything retained for this chain, **in no promised order**.
    ///
    /// Deliberate: the detector sorts what it reads, and promising an order
    /// here is a guarantee a later caller leans on without anything having said
    /// it was made. The returned headers are the caller's to keep.
    ///
    /// # Errors
    ///
    /// [`Error`] when the window cannot be read.
    async fn load(&self, chain_id: u32) -> Result<Vec<BlockHeader>, Error>;

    /// Retain `header`, then discard everything strictly below `retain_from`,
    /// which itself survives.
    ///
    /// **Upserts by `(chain_id, header.number)`.** Two rows at one height would
    /// make the ancestor walk ambiguous, and it would pick whichever the
    /// storage happened to hand back first.
    ///
    /// Pruning rides along rather than being a second call, so there is no
    /// window in which retention is unbounded. `retain_from <= header.number`
    /// is the caller's precondition.
    ///
    /// # Errors
    ///
    /// [`Error`] when the window cannot be written. Nothing is retained then —
    /// reporting a header it does not hold is the one thing this must not do.
    async fn append(
        &self,
        chain_id: u32,
        header: &BlockHeader,
        retain_from: u64,
    ) -> Result<(), Error>;

    /// Discard every header strictly above `last_valid_block`, which survives.
    /// `None` clears the window.
    ///
    /// # Errors
    ///
    /// [`Error`] when the window cannot be written.
    async fn truncate(&self, chain_id: u32, last_valid_block: Option<u64>) -> Result<(), Error>;
}
