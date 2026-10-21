//! Where the indexer has got to, and the loop that gets it there.
//!
//! Everything that does not touch the chain is here and implemented: the
//! cursor, the reorg detector's window, and their durable adapters. Everything
//! that does is a port — [`ChainClient`], [`LogReader`], [`BlockProcessor`] —
//! because an adapter for one links a provider and an HTTP transport, and
//! `./scripts/binary-boundaries.sh` fails if either reaches `bins/api`, which
//! links this crate for [`SyncStatusStore`].

mod chain;
mod cursor;
#[cfg(test)]
mod doubles;
#[cfg(test)]
mod fixtures;
mod processors;
mod reorg;
#[cfg(test)]
mod scratch;

pub use chain::{BlockHeader, ChainClient, Error as ChainError, LogFilter, LogReader, RawLog};
pub use cursor::{
    Cursor, CursorStore, Error, PostgresCursorStore, PostgresSyncStatusStore, SyncStatus,
    SyncStatusStore,
};
pub use processors::{BlockProcessor, Failure, Outcome};
pub use reorg::{
    BlockHeaderStore, Error as ReorgError, HashChainReorgDetector, PostgresBlockHeaderStore,
    ReorgDetector, Verdict,
};
