//! Where the indexer has got to, and — from Phase 3 — the loop that gets it
//! there.
//!
//! Nothing here may reach the chain or a socket: `bins/api` links this crate
//! for [`SyncStatusStore`] alone, and `./scripts/binary-boundaries.sh` fails if
//! a provider or HTTP transport reaches it.
//!
//! The read half of the cursor row only. `CursorStore`, the pipeline's single
//! commit point, arrives with the loop.

mod cursor;

pub use cursor::{Error, PostgresSyncStatusStore, SyncStatus, SyncStatusStore};
