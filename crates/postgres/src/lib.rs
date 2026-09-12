//! Connecting to Postgres.
//!
//! Connection policy and nothing else: how a pool is built, how big it is, how
//! one connection is taken from it, and what a dead server looks like. One way
//! in — [`build_pool`], then [`connection`] for each connection wanted, one for
//! a job that applies migrations and exits, one per request for a service.
//!
//! **What it exports is what its own signatures mention**, which is the line
//! that keeps it honest. `tokio_postgres::Row` appears in none of them — it is
//! what the driver hands a caller that runs a query — so a store adapter names
//! `tokio-postgres` itself rather than reaching for it through here. Routing it
//! through would isolate nobody from anything and would leave this crate
//! claiming a surface it does not own.

mod error;
mod health;
mod pool;

/// The driver's client, which a [`Connection`] derefs to.
///
/// An alias, not a wrapper: refinery implements its traits for this exact type,
/// so anything of ours in between would have to delegate them straight back.
pub type Client = tokio_postgres::Client;

pub use error::Error;
pub use health::ping;
pub use pool::{Connection, Pool, build_pool, connection};
