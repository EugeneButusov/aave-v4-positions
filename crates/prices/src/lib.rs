//! What Aave prices each reserve at.
//!
//! **Enrichment, not fold.** No Aave event carries a price (§4.4), so this is
//! read from the Spoke's own oracle and kept beside the event log rather than
//! derived from it — the same sense in which `token-metadata` is enrichment,
//! and the second thing here that is fetched rather than indexed.
//!
//! **The read half only.** `put`, the refresher and the oracle reader belong to
//! Phase 4; nothing in this workspace writes a price yet.

mod error;
mod postgres;
mod price;
mod store;

#[cfg(test)]
mod conformance;
#[cfg(test)]
mod harness;

pub use error::Error;
pub use postgres::PostgresReservePriceStore;
pub use price::{ReserveKey, ReservePrice};
pub use store::ReservePriceStore;
