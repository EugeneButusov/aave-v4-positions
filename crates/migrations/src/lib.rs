//! Every `.sql` file this deployment applies, embedded at compile time.
//!
//! **Two lists, never one.** The ordinals are unique within a database and not
//! across them — `001_spoke_events` and `001_indexer_cursor` both exist — so
//! concatenating these would collide, and then try to apply Postgres DDL to
//! ClickHouse.
//!
//! Grouped by the directory each came from, rather than flattened, because a
//! group is what a crate would own if the schema ever followed its code. It
//! does not: `bins/migrate` embeds this to apply it and `aave-positions` reads
//! it to build a scratch database, and putting a group inside the crate that
//! uses it would make the migrator link that crate — the read store, alloy and
//! all — to reach a table of string literals. The directory travels with its
//! list so the completeness test can hold the two to each other.
//!
//! **A corpus, not a runner.** Nothing here knows what applying a migration
//! means, which is why this crate has no dependencies.
//!
//! The paths reach into `packages/` because the TypeScript runner reads the
//! same files. They come in here when `packages/` goes in Phase 5.

mod clickhouse;
#[cfg(test)]
mod completeness;
mod embedded;
mod postgres;

pub use clickhouse::CLICKHOUSE;
pub use embedded::{Embedded, Source};
pub use postgres::POSTGRES;
