use alloy_primitives::B256;
use async_trait::async_trait;

use crate::chain::BlockHeader;
use crate::cursor::Cursor;
use crate::reorg::Error;

/// What the detector says about a header, or about a resume point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Continuous,

    /// Both ends inclusive. `last_valid_hash` is the hash of
    /// `first_invalid_block - 1`, carried so the rewound cursor needs no second
    /// read that another fork could race.
    Reorg {
        first_invalid_block: u64,
        last_invalid_block: u64,
        last_valid_hash: B256,
    },

    /// The fork reaches past what is retained, or past what was ever inspected.
    Unrecoverable {
        reason: String,
    },
}

/// The chain's shape, behind one port.
///
/// The loop computes no finality boundary of its own: it asks
/// [`safe_head`](Self::safe_head), decides how wide to dispatch from the answer,
/// and hands the same answer to [`inspect`](Self::inspect) and
/// [`commit`](Self::commit).
#[async_trait]
pub trait ReorgDetector: Send + Sync {
    /// Vet the resume point against the chain, once, before the first
    /// iteration.
    ///
    /// May refill its window, but **only downwards from a block just proven
    /// canonical, checking each `parent_hash` link**. Refilling by height past
    /// a reorged-out resume point records the branch that won and erases the
    /// only evidence of the one that lost.
    ///
    /// # Errors
    ///
    /// [`Error`] when the window or the chain cannot be read. A transient
    /// failure is not a verdict; the caller runs the whole step again.
    async fn bootstrap(&self, cursor: Option<&Cursor>) -> Result<Verdict, Error>;

    /// The highest block this detector calls settled. `None` means nothing is,
    /// which is the answer on a chain shorter than the depth.
    fn safe_head(&self, observed_head: u64) -> Option<u64>;

    /// Vet one header against what is retained below it. `safe_head` is what
    /// [`safe_head`](Self::safe_head) answered for this iteration's head.
    ///
    /// `header.number` is at most one above the highest committed and **may
    /// equal it**, which a rejected cursor save makes ordinary: an
    /// implementation that asserted strict succession would panic on a replay.
    ///
    /// # Errors
    ///
    /// [`Error`] when the window or the chain cannot be read.
    async fn inspect(&self, header: &BlockHeader, safe_head: Option<u64>)
    -> Result<Verdict, Error>;

    /// Called only after every processor accepted the range, and **always
    /// before the cursor is saved**. `safe_head` as for [`inspect`](Self::inspect).
    ///
    /// # Errors
    ///
    /// [`Error`] when the window cannot be written.
    async fn commit(&self, header: &BlockHeader, safe_head: Option<u64>) -> Result<(), Error>;

    /// Discard the retained headers above `last_valid_block`; `None` clears the
    /// window.
    ///
    /// **Called after the rewound cursor is saved, never before.** Those
    /// headers are the evidence of the abandoned branch, so a crash between the
    /// two must leave the window ahead of the cursor rather than behind it —
    /// which is the state [`bootstrap`](Self::bootstrap) resolves.
    ///
    /// # Errors
    ///
    /// [`Error`] when the window cannot be written.
    async fn rewind_to(&self, last_valid_block: Option<u64>) -> Result<(), Error>;
}
