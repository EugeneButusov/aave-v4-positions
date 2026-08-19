//! Connecting to Postgres.
//!
//! Connectivity only, for the reason `clickhouse-client` gives.

mod connect;
mod error;

/// `connect` hands back a `tokio_postgres::Client`, which a caller cannot name
/// without this.
pub use tokio_postgres;

pub use connect::connect;
pub use error::Error;
