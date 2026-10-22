use alloy_primitives::B256;
use time::OffsetDateTime;
use units::{ChainId, UnixSeconds};

/// How far the indexer has got on one chain, and how long ago it got there.
///
/// The same row `CursorStore` will describe, read back by something that is not
/// the indexer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncStatus {
    pub chain_id: ChainId,
    pub last_block: u64,
    pub last_hash: B256,

    /// Written by the database's clock.
    pub updated_at: OffsetDateTime,

    /// What the chain said the time was at [`Self::last_block`].
    ///
    /// `None` is a height the retention window has no header for, which it
    /// passes through while the indexer rebuilds the window below a new top.
    pub last_block_timestamp: Option<UnixSeconds>,

    /// Whole seconds since the indexer last advanced, **measured by the
    /// database's own clock**.
    ///
    /// Not derivable from [`Self::updated_at`] by a reader: that timestamp is
    /// written by the database, so subtracting it from a different process's
    /// clock reports skew as staleness — and in the direction that matters, a
    /// fast reader declares a healthy indexer stale.
    pub age_seconds: u64,
}
