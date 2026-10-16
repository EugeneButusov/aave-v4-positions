//! Every `.sql` file this deployment applies, embedded at compile time.
//!
//! Two lists, never one: the ordinals are unique within a database and not
//! across them, so concatenating them would collide and then apply Postgres DDL
//! to ClickHouse. Each list carries the directory it came from, which is what
//! the completeness test holds it to.
//!
//! A corpus, not a runner — which is why this crate has no dependencies.
//!
//! The paths reach into `packages/` because the TypeScript runner reads the
//! same files; they move here when `packages/` goes in Phase 5.

mod clickhouse;
#[cfg(test)]
mod completeness;
mod embedded;
mod postgres;

pub use clickhouse::CLICKHOUSE;
pub use embedded::{Embedded, Source};
pub use postgres::POSTGRES;
