//! Connecting to Postgres.
//!
//! Connectivity, for the reason `clickhouse-client` gives, and the two reads
//! that carry a span. One way in: [`build_pool`], then [`connection`] for each
//! connection wanted — one, for a job that applies migrations and exits, or one
//! per request for a service.
//!
//! **[`Traced`] is here because the span cannot be**, and that is the only
//! reason: `tokio-postgres` opens none, so a read issued straight against the
//! driver is a silent gap in a trace. A caller that reaches past it gets the
//! driver's own `query` and no span, which the `_traced` suffix is there to make
//! visible at the call site rather than only in the imports.

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
pub use query::Traced;
