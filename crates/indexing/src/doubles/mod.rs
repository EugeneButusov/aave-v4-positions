//! Stand-ins for the ports, for cases that are about something else.
//!
//! Each runs its port's conformance suite, because a double that quietly
//! disagreed with its port would turn every case using it into a proof about a
//! fiction. Nothing re-exports them yet: each is driven by its own cases, and
//! the loop that holds them all arrives next.

mod block_headers;
mod cursor_store;
