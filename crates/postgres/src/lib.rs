//! Connecting to Postgres.
//!
//! Connectivity only, for the reason `clickhouse-client` gives.

mod connect;
mod error;

pub use connect::connect;
pub use error::Error;
