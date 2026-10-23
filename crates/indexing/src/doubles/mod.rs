//! Stand-ins for the ports, for cases that are about something else.
//!
//! Each runs its port's conformance suite: a double that quietly disagreed with
//! its port would turn every case using it into a proof about a fiction.

mod block_headers;
mod cursor_store;
