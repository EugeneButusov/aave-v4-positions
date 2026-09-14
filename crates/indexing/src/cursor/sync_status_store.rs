use crate::cursor::{Error, SyncStatus};

/// Reads the indexer's position from outside the indexer.
///
/// **A separate port from `CursorStore`, over the same row, on purpose.** That
/// one is the pipeline's single commit point — a write seam the loop owns,
/// whose whole contract is about ordering and durability. This is a read-only
/// view for a process that does not index and must not write: the API stamps
/// every payload with it, because amounts and health factors are per-block
/// quantities (§12.6) and a number with no block behind it cannot be checked.
#[async_trait::async_trait]
pub trait SyncStatusStore: Send + Sync {
    /// `None` when this chain has never been indexed.
    ///
    /// **Which the read path treats as a missing resource, not an empty page.**
    /// A chain with no cursor row is one this deployment does not serve, and
    /// answering `200` with nothing in it would say the wallet holds no
    /// positions — a different claim, and a wrong one.
    ///
    /// # Errors
    ///
    /// [`Error`] when the query cannot be taken or the row cannot be read.
    async fn get(&self, chain_id: u32) -> Result<Option<SyncStatus>, Error>;
}
