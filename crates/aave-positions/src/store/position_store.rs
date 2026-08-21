//! The port: what a read asks for, what it gets back, and the one method
//! between them.
//!
//! `bins/api` will hold this as `Arc<dyn PositionStore>` rather than a generic,
//! so the composition root reads like the module graph it replaces — which is
//! why it carries `#[async_trait]`. `async fn` in a trait is stable and still
//! not dyn compatible: measured on 1.96, `Arc<dyn PositionStore>` over a plain
//! `async fn list` is E0038, and the compiler's own advice is to use the
//! concrete type instead. A boxed future per page is the price of the seam.
//!
//! No `Send + Sync` supertraits until something holds one as state and needs
//! them.

use alloy_primitives::{Address, U256};
use async_trait::async_trait;

use super::{Error, Position};

/// Where a page stopped: the sorting key below `(chain_id, user)`, and nothing
/// else.
///
/// A plain key rather than an opaque token. Signing one so a caller cannot
/// forge or carry it between listings is a property of *publishing* it, not of
/// paging, and it lives with whoever publishes it — for the HTTP API, in
/// `bins/api`. Nothing in this crate holds a signing key or knows what a cursor
/// looks like.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionKey {
    pub spoke: Address,
    pub reserve_id: U256,
}

/// One wallet's positions, on one Spoke or on all of them.
///
/// **`user` is required**, which makes this a lookup rather than a scan: with
/// `chain_id` it is the leading pair of the sorting key, so pinning it turns
/// every page into a seek into contiguous rows.
///
/// **`spoke` is optional, and what it narrows is the listing, never the
/// arithmetic.** A Spoke is an isolated margin account with its own collateral
/// factors, oracle and health factor (§12.3), so *summing* across two of them
/// is wrong in the one direction that matters — it hides an imminent
/// liquidation behind unrelated collateral. Listing them together is fine,
/// because every row names the Spoke it came from. Nothing here is aggregated,
/// and anything that ever aggregates must do it per Spoke.
///
/// Cross-wallet questions ("largest open positions") are analytics over the
/// same view, not a mode of this port.
#[derive(Debug, Clone)]
pub struct PositionQuery {
    pub chain_id: u32,
    pub user: Address,
    /// `None` lists every Spoke this wallet has touched.
    pub spoke: Option<Address>,
    pub limit: u32,
    /// Resume point, already verified by whoever owns the wire format.
    pub after: Option<PositionKey>,
    /// Unix seconds to value the page at. `None` is now.
    ///
    /// Amounts are a per-second quantity — the Hub's index accrues continuously
    /// and emits nothing (§5) — so "now" is a real choice rather than an
    /// absence of one, and it is the same choice the chain makes:
    /// `getUserDebt` at `latest` is stored shares times an index extrapolated
    /// to the head block. Naming it explicitly is what makes a response
    /// reproducible, and what lets reconciliation pin both sides to one block.
    ///
    /// The shares are as of whatever the indexer has folded, which is not the
    /// same clock. [`PositionPage::valued_at`] reports this one so the two are
    /// not silently conflated.
    pub as_of: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionPage {
    pub items: Vec<Position>,
    /// Unix seconds every amount on this page was computed at — one instant for
    /// the whole page, so two positions in it cannot disagree about the time.
    pub valued_at: u64,
    /// `None` on the last page. Absence is the end, not an empty key.
    pub next: Option<PositionKey>,
}

/// Reads the folded positions.
///
/// **Read-only by construction.** The fold is materialized views over the
/// event ledger, so positions advance because the indexer appended events —
/// there is no ingestion path here to keep in step with one, and a reorg
/// repairs the projection without an implementation of this learning of it.
///
/// **Only open positions.** §12.1: a position exists while its shares are
/// non-zero. A closed one keeps its row and its event count, and is filtered
/// out by the implementation rather than deleted anywhere.
#[async_trait]
pub trait PositionStore {
    /// One wallet's open positions, valued at one instant.
    async fn list(&self, query: &PositionQuery) -> Result<PositionPage, Error>;
}
