//! Reading a container's environment, one variable at a time.
//!
//! Nothing here knows what a service is: it knows a port is `1..=65535` and
//! that `pino` spells a level seven ways. What those values mean to a running
//! process belongs to a binary's `config`.
//!
//! [`Source`] says where the variables come from, [`Env`] reads them one at a
//! time and reports the one it could not read as [`Invalid`].

mod reader;
mod source;

pub use reader::{Env, Invalid};
pub use source::Source;
