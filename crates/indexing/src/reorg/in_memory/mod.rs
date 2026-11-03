//! The in-memory implementation of [`BlockHeaderStore`](super::BlockHeaderStore).

mod block_header_store;

pub(crate) use block_header_store::InMemoryBlockHeaderStore;
