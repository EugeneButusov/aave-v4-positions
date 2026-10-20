//! What a case needs that the binary does not.
//!
//! Two kinds of thing, kept apart because they share nothing: [`app`] stands up
//! an `App` with its four ports doubled, and [`observability`] reads back what
//! one request produced. The path a case imports says which it is using.

pub(crate) mod app;
pub(crate) mod observability;
