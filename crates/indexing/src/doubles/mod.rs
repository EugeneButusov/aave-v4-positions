//! Stand-ins for the ports, for cases that are about something else.
//!
//! Each runs its port's conformance suite where it has one, because a double
//! that quietly disagreed with its port would turn every case using it into a
//! proof about a fiction.

mod block_headers;
mod cursor_store;
mod fake_chain;
mod forking_chain;
mod windows;

pub(crate) use block_headers::InMemoryBlockHeaderStore;
pub(crate) use fake_chain::{ChainCall, FakeChain};
pub(crate) use forking_chain::ForkingChain;
pub(crate) use windows::{RecordingBlockHeaderStore, ShufflingBlockHeaderStore};

/// So a case can keep the store it also handed to the detector.
///
/// The port is held as `Box<dyn _>` by its one owner, which is right for the
/// binary and leaves a case nothing to read the window through. This is the
/// accommodation, and it lives here with the other test machinery.
#[async_trait::async_trait]
impl<S: crate::reorg::BlockHeaderStore + ?Sized> crate::reorg::BlockHeaderStore
    for std::sync::Arc<S>
{
    async fn load(
        &self,
        chain_id: u32,
    ) -> Result<Vec<crate::chain::BlockHeader>, crate::reorg::Error> {
        (**self).load(chain_id).await
    }

    async fn append(
        &self,
        chain_id: u32,
        header: &crate::chain::BlockHeader,
        retain_from: u64,
    ) -> Result<(), crate::reorg::Error> {
        (**self).append(chain_id, header, retain_from).await
    }

    async fn truncate(
        &self,
        chain_id: u32,
        last_valid_block: Option<u64>,
    ) -> Result<(), crate::reorg::Error> {
        (**self).truncate(chain_id, last_valid_block).await
    }
}
