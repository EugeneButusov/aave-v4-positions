//! The port: what a read asks for, what it gets back, and the one method
//! between them.
//!
//! `#[async_trait]` because `bins/api` holds this as a `Box<dyn PositionStore>`
//! and a plain `async fn` in a trait is still not dyn compatible (E0038 on
//! 1.96). `Send + Sync` because axum clones its state across worker threads.

use alloy_primitives::{Address, U256};
use async_trait::async_trait;
use units::{ChainId, UnixSeconds};

use super::{Error, Position};

/// Where a page stopped: the sorting key below `(chain_id, user)`, and nothing
/// else.
///
/// A plain key rather than an opaque token: signing one belongs to whoever
/// publishes it. Nothing in this crate holds a signing key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionKey {
    pub spoke: Address,
    pub reserve_id: U256,
}

/// One wallet's positions, on one Spoke or on all of them.
///
/// `user` is required, which makes this a lookup rather than a scan: with
/// `chain_id` it is the leading pair of the sorting key.
///
/// `spoke` narrows the listing, never the arithmetic. A Spoke is an isolated
/// margin account (§12.3), so summing across two hides an imminent liquidation
/// behind unrelated collateral; anything that aggregates must do it per Spoke.
#[derive(Debug, Clone)]
pub struct PositionQuery {
    pub chain_id: ChainId,
    pub user: Address,
    /// `None` lists every Spoke this wallet has touched.
    pub spoke: Option<Address>,
    pub limit: u32,
    /// Resume point, already verified by whoever owns the wire format.
    pub after: Option<PositionKey>,
    /// The instant to read and value the page at.
    ///
    /// Amounts are a per-second quantity (§5). Required, not optional: a store
    /// with a default here would be reading its own clock.
    pub cut: UnixSeconds,
}

/// What came back, and nothing about when: a store echoing
/// [`PositionQuery::cut`] back is the one place the stamp and the payload can
/// come to disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionPage {
    pub items: Vec<Position>,
    /// `None` on the last page. Absence is the end, not an empty key.
    pub next: Option<PositionKey>,
}

/// Reads the folded positions.
///
/// Read-only by construction: the fold is materialized views over the event
/// ledger, so a reorg repairs the projection without this learning of it.
///
/// Open positions only (§12.1). A closed one keeps its row and its event count
/// and is filtered out rather than deleted.
#[async_trait]
pub trait PositionStore: Send + Sync {
    /// One wallet's open positions, valued at one instant.
    async fn list(&self, query: &PositionQuery) -> Result<PositionPage, Error>;
}
