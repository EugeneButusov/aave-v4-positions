//! Connecting to Postgres.
//!
//! Connectivity only, for the reason `clickhouse-client` gives. One way in:
//! [`build_pool`], then [`connection`] for each connection wanted — one, for a
//! job that applies migrations and exits, or one per request for a service.

mod error;
mod health;
mod pool;

/// The driver itself, re-exported as `clickhouse-client` re-exports `clickhouse`.
///
/// A store in another crate reads its rows with the driver's own `Row` and
/// reports the driver's own error; taking those through here is what keeps one
/// version of it in the workspace rather than each adapter naming its own.
pub use tokio_postgres;

/// The driver's client, which a [`Connection`] derefs to.
///
/// An alias, not a wrapper: refinery implements its traits for this exact type,
/// so anything of ours in between would have to delegate them straight back.
pub type Client = tokio_postgres::Client;

pub use error::Error;
pub use health::ping;
pub use pool::{Connection, Pool, build_pool, connection};
