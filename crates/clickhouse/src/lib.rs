//! The ClickHouse client this repository uses.
//!
//! Connectivity, including the sanitized query readiness reports. Anything
//! that knows what a migration is lives with the binary that applies them.

mod client;
mod error;
mod health;

/// The driver, so a caller naming a `Client` needs one dependency and cannot
/// end up on a different version than this crate was built against.
pub use clickhouse;
pub use client::{Config, TransportSecurity, build_client};
pub use error::Error;
pub use health::ping;
