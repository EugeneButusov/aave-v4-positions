//! What Aave prices each reserve at.
//!
//! Enrichment, not fold: no Aave event carries a price (§4.4), so this is read
//! from the Spoke's own oracle and kept beside the event log.
//!
//! The read half only — `put`, the refresher and the oracle reader are Phase 4.
//!
//! The port is this level; the adapter and its harness are a directory down.

mod error;
mod postgres;
mod price;
mod reserve_price_store;

#[cfg(test)]
mod conformance;

pub use error::Error;
pub use postgres::PostgresReservePriceStore;
pub use price::{ReserveKey, ReservePrice};
pub use reserve_price_store::ReservePriceStore;
