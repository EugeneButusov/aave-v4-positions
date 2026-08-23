//! Connecting to Postgres.
//!
//! Connectivity only, for the reason `clickhouse-client` gives. Two ways in:
//! [`build_client`] for a process that runs to completion, [`build_pool`] for
//! one that serves.

mod client;
mod error;
mod health;
mod pool;

pub use client::{Client, build_client};
pub use error::Error;
pub use health::ping;
pub use pool::{Pool, build_pool};
