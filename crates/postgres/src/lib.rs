//! Connecting to Postgres.
//!
//! Connectivity, for the reason `clickhouse-client` gives, the two reads that
//! carry a span, and the one place this database's signedness is answered.
//! One way in: [`build_pool`], then [`TracedPool`] for a read or
//! [`connection`] for the rest of the driver's surface.

mod bigint;
mod error;
mod health;
mod pool;
mod query;
mod tls;

/// The driver's client, which a [`Connection`] derefs to.
///
/// An alias, not a wrapper: refinery implements its traits for this exact type,
/// so anything of ours in between would have to delegate them straight back.
pub type Client = tokio_postgres::Client;

pub use bigint::Bigint;
pub use error::Error;
pub use health::ping;
pub use pool::{Connection, Deadline, Limits, Pool, build_pool, connection};
pub use query::TracedPool;
