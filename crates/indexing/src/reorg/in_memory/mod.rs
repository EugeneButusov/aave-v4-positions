//! The in-memory implementations of [`BlockHeaderStore`](super::BlockHeaderStore).
//!
//! Two of them wrap the third: one records and can refuse a chosen write, one
//! hands the window back in an order the port does not promise.

mod block_header_store;

pub(crate) use block_header_store::{
    InMemoryBlockHeaderStore, RecordingBlockHeaderStore, ShufflingBlockHeaderStore,
};
