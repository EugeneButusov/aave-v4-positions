//! Reading a container's environment: one variable at a time, collecting the
//! problems rather than stopping at the first.
//!
//! **Nothing here knows what a service is**, and that is the whole boundary. It
//! knows a port is `1..=65535` and that `pino` spells a level seven ways. What
//! those values *mean* to a running process — which one the listener binds,
//! which one the shutdown waits out — belongs to a binary's `config`, along with
//! the mapping from variable name to field.
//!
//! A crate rather than a module because more than one binary boots this way:
//! `bins/api` today, `bins/indexer` and its five CLIs next. It is the same
//! reason `ops` is a crate, and the same line crates.io draws with
//! `crates_io_env_vars`, which every one of its `config/*.rs` modules calls
//! into.
//!
//! Two halves, and a caller uses them in this order: [`Source`] says where the
//! variables come from, [`Env`] reads them one at a time and reports everything
//! wrong with them as [`Invalid`].

mod reader;
mod source;

pub use reader::{Env, Invalid};
pub use source::Source;
