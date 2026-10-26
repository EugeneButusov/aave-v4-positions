#[cfg(test)]
use std::sync::Arc;

use async_trait::async_trait;
use units::ChainId;

use crate::chain::BlockHeader;
use crate::reorg::Error;

/// The retention window: one contiguous run of headers per chain, the deepest
/// recoverable fork's worth.
///
/// **Durable, and there is only one adapter.** An in-memory window under a
/// durable cursor is worse than neither — the resume point is exactly what a
/// fork reorged out, so every fork across a restart reads as unrecoverable.
#[async_trait]
pub trait BlockHeaderStore: Send + Sync {
    /// Everything retained for this chain, **in no promised order**, and the
    /// caller's to keep. The detector sorts what it reads.
    ///
    /// # Errors
    ///
    /// [`Error`] when the window cannot be read.
    async fn load(&self, chain_id: ChainId) -> Result<Vec<BlockHeader>, Error>;

    /// Retain `header`, then discard everything strictly below `retain_from`,
    /// which itself survives. `retain_from <= header.number` is the caller's
    /// precondition.
    ///
    /// **Upserts by `(chain_id, header.number)`.** Two rows at one height make
    /// the ancestor walk pick whichever the storage hands back first.
    ///
    /// # Errors
    ///
    /// [`Error`] when the window cannot be written. Nothing is retained then:
    /// reporting a header it does not hold is the one thing this must not do.
    async fn append(
        &self,
        chain_id: ChainId,
        header: &BlockHeader,
        retain_from: u64,
    ) -> Result<(), Error>;

    /// Discard every header strictly above `last_valid_block`, which survives.
    /// `None` clears the window.
    ///
    /// # Errors
    ///
    /// [`Error`] when the window cannot be written.
    async fn truncate(&self, chain_id: ChainId, last_valid_block: Option<u64>)
    -> Result<(), Error>;
}

/// So a case can keep the window it handed to the detector as a `Box<dyn _>`.
#[cfg(test)]
#[async_trait]
impl<S: BlockHeaderStore + ?Sized> BlockHeaderStore for Arc<S> {
    async fn load(&self, chain_id: ChainId) -> Result<Vec<BlockHeader>, Error> {
        (**self).load(chain_id).await
    }

    async fn append(
        &self,
        chain_id: ChainId,
        header: &BlockHeader,
        retain_from: u64,
    ) -> Result<(), Error> {
        (**self).append(chain_id, header, retain_from).await
    }

    async fn truncate(
        &self,
        chain_id: ChainId,
        last_valid_block: Option<u64>,
    ) -> Result<(), Error> {
        (**self).truncate(chain_id, last_valid_block).await
    }
}
