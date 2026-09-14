//! The port: what a caller asks for, and what it gets back.

use std::collections::HashMap;

use crate::{Error, ReserveKey, ReservePrice};

/// Reads what Aave prices each reserve at.
///
/// **No pagination.** A Spoke lists fourteen reserves on mainnet, the whole
/// dimension fits in one response, and the read path wants all of it at once.
#[async_trait::async_trait]
pub trait ReservePriceStore: Send + Sync {
    /// Every price on this chain, keyed by [`ReserveKey`].
    ///
    /// **The whole dimension, deliberately not the reserves on a page.** Taking
    /// a page's keys would make this depend on the page and force it to run
    /// after the ClickHouse query; keyed only by chain it runs *beside* it, in
    /// the same `join!` the labels read already uses — a third round trip that
    /// costs no wall clock.
    ///
    /// A reserve with no row is **absent from the map**, and the read path
    /// serves null for it rather than a zero. A price of zero is not a price:
    /// the oracle reverts on one (§7.4), so a zero here could only ever be our
    /// own invention.
    ///
    /// # Errors
    ///
    /// [`Error`] when the query cannot be taken or a row cannot be read.
    async fn latest(&self, chain_id: u32) -> Result<HashMap<ReserveKey, ReservePrice>, Error>;
}
