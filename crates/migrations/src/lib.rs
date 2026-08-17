//! Compile-time embedded `.sql` migrations.
//!
//! A crate that owns tables keeps its `.sql` beside it and declares its own set,
//! so adding a table is one crate's business and this crate never becomes a
//! catalogue of every table in the system. `bins/migrate` is handed the union,
//! one list per database.
//!
//! ```ignore
//! pub const MIGRATIONS: &[Migration] = &[
//!     Migration::new("001_spoke_events", include_str!("migrations/001_spoke_events.sql")),
//!     Migration::new("002_spoke_events_current", include_str!("migrations/002_spoke_events_current.sql")),
//! ];
//! ```
//!
//! **One `include_str!` per file, written by hand.** Embedded rather than read at
//! run time because the binary ships on its own, with no directory beside it to
//! read. Not `include_dir!`: it is a proc macro, the `proc_macro::tracked_path`
//! API it would need is nightly-only, and without it an edited migration does not
//! trigger a rebuild — a warm `target/` ships yesterday's SQL and says nothing.
//! Not a globbing macro either, since adding a file changes no Rust source and
//! the macro would not re-run. launchbadge/sqlx#681 is the reference for both
//! halves.
//!
//! Because the set is a compile-time constant, the checks over it — that ordinals
//! ascend, and that the list and the directory still agree — are tests rather than
//! runtime guards. They settle in CI and cannot fire in front of a user.

mod migration;
mod statements;

pub use migration::Migration;

// The checks over a migration set, each with the tests that run it. Lift them out
// once a crate that owns .sql files needs them; until then they are assertions
// about a constant, not something a service calls.
#[cfg(test)]
mod completeness;
#[cfg(test)]
mod ordering;
