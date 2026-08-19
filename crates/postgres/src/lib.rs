//! Connecting to Postgres.
//!
//! Connectivity only, for the reason `clickhouse-client` gives.

mod client;
mod error;

pub use client::build_client;
pub use error::Error;
