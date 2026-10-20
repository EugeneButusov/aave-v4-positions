//! What a case needs that the binary does not: an app with its ports doubled,
//! and a way to read back what one request produced.

mod app;
mod observability;

pub(crate) use app::{
    Stores, VALUED_AT, answered, get, handler, postgres, state, synced, unreachable_postgres,
};
pub(crate) use observability::{Written, interesting, observed};
