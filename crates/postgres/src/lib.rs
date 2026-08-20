//! Connecting to Postgres.
//!
//! Connectivity only, for the reason `clickhouse-client` gives.

mod client;
mod error;

pub use client::{Client, build_client};
pub use error::Error;
