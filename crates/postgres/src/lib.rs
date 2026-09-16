//! Connecting to Postgres.
//!
//! Connectivity, for the reason `clickhouse-client` gives, and the two reads
//! that carry a span. One way in: [`build_pool`], then [`connection`] for each
//! connection wanted — one, for a job that applies migrations and exits, or one
//! per request for a service.
//!
//! **[`query`] and [`query_opt`] are here because the span cannot be**, and that
//! is the only reason: `tokio-postgres` opens none, so a read issued straight
//! against the driver is a silent gap in a trace. A caller that reaches past
//! these gets the driver's own `Client` and no span, which is the shape this
//! exists to make deliberate rather than accidental.

mod error;
mod health;
mod pool;
mod query;

/// The driver's client, which a [`Connection`] derefs to.
///
/// An alias, not a wrapper: refinery implements its traits for this exact type,
/// so anything of ours in between would have to delegate them straight back.
pub type Client = tokio_postgres::Client;

pub use error::Error;
pub use health::ping;
pub use pool::{Connection, Pool, build_pool, connection};
pub use query::{Statement, query, query_opt};
