//! Where the indexer has got to, and — from Phase 3 — the loop that gets it
//! there.
//!
//! **Nothing here may reach the chain or a socket.** `bins/api` links this
//! crate for [`SyncStatusStore`] alone, and `docs/rust-migration.md` forbids
//! the read API a provider or an HTTP transport. That rule is about what ends
//! up in the binary rather than about a crate name: `alloy-primitives` is a
//! hash and an integer, which `api` already links through `aave-positions`, so
//! it is fine here. `alloy-provider` and `alloy-transport-http` are not, and
//! when Phase 3 brings the loop the adapters that need them stay outside this
//! crate — otherwise the read API grows an HTTP client it never calls, and
//! CI's `cargo tree -i` says so.
//!
//! **The read half of the cursor row only.** `CursorStore` — the pipeline's
//! single commit point, whose contract is ordering and durability — arrives
//! with the loop. [`SyncStatusStore`] is the other side of the same row: a
//! read-only view for a process that does not index and must not write.

mod cursor;

pub use cursor::{Error, PostgresSyncStatusStore, SyncStatus, SyncStatusStore};
