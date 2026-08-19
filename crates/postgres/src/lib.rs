//! Connecting to Postgres.
//!
//! Connectivity only, for the reason `clickhouse-client` gives.

mod client;
mod error;

/// `build_client` hands back a `tokio_postgres::Client`, which a caller cannot
/// name without this.
pub use tokio_postgres;

pub use client::build_client;
pub use error::Error;
