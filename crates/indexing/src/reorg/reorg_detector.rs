use alloy_primitives::B256;
use async_trait::async_trait;

use crate::chain::BlockHeader;
use crate::cursor::Cursor;
use crate::reorg::Error;

/// What the detector says about a header, or about a resume point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// It follows what came before.
    Continuous,

    /// Both ends inclusive. `last_valid_hash` is the hash of
    /// `first_invalid_block - 1`, carried so the rewound cursor can be written
    /// without a second read that could race another fork.
    Reorg {
        first_invalid_block: u64,
        last_invalid_block: u64,
        last_valid_hash: B256,
    },

    /// The fork reaches past what is retained, or past what was ever inspected.
    /// Reported rather than guessed at: reconciliation is what catches this
    /// class of corruption.
    Unrecoverable { reason: String },
}

/// The chain's shape, behind one port.
///
/// The loop holds no notion of finality and computes no boundary. It asks
/// [`safe_head`](Self::safe_head) and uses the answer for one thing — how wide
/// to dispatch — which is what keeps a finality judgement out of the component
/// that is supposed to hold none.
#[async_trait]
pub trait ReorgDetector: Send + Sync {
    /// Vet the resume point against the chain, once, before the first
    /// iteration.
    ///
    /// May refill its window, but **only downwards from a block just proven
    /// canonical, checking each `parent_hash` link**. Reading headers by height
    /// past a reorged-out resume point records the branch that won and erases
    /// the only evidence of the branch that lost.
    ///
    /// # Errors
    ///
    /// [`Error`] when the window or the chain cannot be read. A transient
    /// failure is not a verdict — the caller runs the whole step again.
    async fn bootstrap(&self, cursor: Option<&Cursor>) -> Result<Verdict, Error>;

    /// The highest block this detector will call settled, given a just-observed
    /// head. `None` means treat everything as unsettled, which is the honest
    /// answer on a chain shorter than the depth.
    ///
    /// # Errors
    ///
    /// [`Error`] when answering needs a read that fails.
    async fn safe_head(&self, observed_head: u64) -> Result<Option<u64>, Error>;

    /// Vet one header against what is retained below it.
    ///
    /// `header.number` is at most one above the highest committed and **may
    /// equal it**: a rejected cursor save replays that block. Equality is
    /// ordinary and must not be treated as an assertion failure.
    ///
    /// # Errors
    ///
    /// [`Error`] when the window or the chain cannot be read.
    async fn inspect(&self, header: &BlockHeader) -> Result<Verdict, Error>;

    /// Called only after every processor accepted the range, and **always
    /// before the cursor is saved**.
    ///
    /// # Errors
    ///
    /// [`Error`] when the window cannot be written.
    async fn commit(&self, header: &BlockHeader) -> Result<(), Error>;

    /// Discard the retained headers above `last_valid_block`; `None` clears the
    /// window.
    ///
    /// **Called after the rewound cursor is saved, never before.** Those
    /// headers are the evidence of the branch being abandoned, so discarding
    /// them for a write that has not landed gets the order backwards. A crash
    /// between the two leaves the window ahead of the cursor, which
    /// [`bootstrap`](Self::bootstrap) is built to resolve.
    ///
    /// # Errors
    ///
    /// [`Error`] when the window cannot be written. The reorg is then still
    /// owed, and the cursor and the window say so between them.
    async fn rewind_to(&self, last_valid_block: Option<u64>) -> Result<(), Error>;
}
