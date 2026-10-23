//! A migrated Postgres schema per case, so cases can run at once.
//!
//! Only ever a dev-dependency. Four harnesses wanted the same twenty lines,
//! and neither crate that could have held them may take the other's
//! dependency: `postgres` is connectivity and `migrations` has none at all.
#![allow(
    clippy::unwrap_used,
    reason = "a database a case cannot reach is a broken environment, not a \
              runtime path, and the panic is the whole report"
)]

mod corpus;
mod schema;

pub use schema::Scratch;
