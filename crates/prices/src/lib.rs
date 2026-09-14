//! What Aave prices each reserve at.
//!
//! **Enrichment, not fold.** No Aave event carries a price (§4.4), so this is
//! read from the Spoke's own oracle and kept beside the event log rather than
//! derived from it — the same sense in which `token-metadata` is enrichment,
//! and the second thing here that is fetched rather than indexed.
//!
//! **The read half only.** `put`, the refresher and the oracle reader belong to
//! Phase 4; nothing in this workspace writes a price yet.
//!
//! **The port is this level; the adapter is a directory down.** `postgres` is
//! the one implementation, and the harness that stands a scenario up for it
//! lives with it rather than beside the port it is proving.

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
