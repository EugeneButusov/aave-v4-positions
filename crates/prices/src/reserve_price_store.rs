//! The port: what a caller asks for, and what it gets back.

use std::collections::HashMap;

use units::ChainId;

use crate::{Error, ReserveKey, ReservePrice};

/// Reads what Aave prices each reserve at.
///
/// No pagination: a Spoke lists fourteen reserves on mainnet.
#[async_trait::async_trait]
pub trait ReservePriceStore: Send + Sync {
    /// Every price on this chain, keyed by [`ReserveKey`].
    ///
    /// The whole dimension, not the reserves on a page: keyed by chain alone it
    /// runs beside the ClickHouse query, in the same `join!` as the labels.
    ///
    /// A reserve with no row is absent from the map and served as null rather
    /// than a zero, which the oracle reverts on rather than answer (§7.4).
    ///
    /// # Errors
    ///
    /// [`Error`] when the query cannot be taken or a row cannot be read.
    async fn latest(&self, chain_id: ChainId) -> Result<HashMap<ReserveKey, ReservePrice>, Error>;
}
