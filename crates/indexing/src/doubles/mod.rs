//! Stand-ins for the ports, for cases that are about something else.
//!
//! Each runs its port's conformance suite where it has one, because a double
//! that quietly disagreed with its port would turn every case using it into a
//! proof about a fiction.

mod block_headers;
mod cursor_store;
pub(crate) mod detector;
mod fake_chain;
mod forking_chain;
mod processor;
mod windows;

pub(crate) use block_headers::InMemoryBlockHeaderStore;
pub(crate) use cursor_store::RecordingCursorStore;
pub(crate) use detector::{ScriptedDetector, recorded, trace};
pub(crate) use fake_chain::{ChainCall, FakeChain};
pub(crate) use forking_chain::ForkingChain;
pub(crate) use processor::RecordingProcessor;
pub(crate) use windows::{RecordingBlockHeaderStore, ShufflingBlockHeaderStore};

/// So a case can keep the collaborator it also handed over.
///
/// Every port is held as `Box<dyn _>` or `Arc<dyn _>` by its owner, which is
/// right for the binary and leaves a case nothing to read the double through.
/// These are the accommodation, and they live here with the other test
/// machinery rather than widening a port.
macro_rules! delegating_arc_impl {
    ($trait:path { $($signature:item)* }) => {
        #[async_trait::async_trait]
        impl<S: $trait + ?Sized> $trait for std::sync::Arc<S> {
            $($signature)*
        }
    };
}

delegating_arc_impl!(crate::reorg::BlockHeaderStore {
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
});

delegating_arc_impl!(crate::reorg::ReorgDetector {
    async fn bootstrap(
        &self,
        cursor: Option<&crate::cursor::Cursor>,
    ) -> Result<crate::reorg::Verdict, crate::reorg::Error> {
        (**self).bootstrap(cursor).await
    }
    async fn safe_head(&self, observed_head: u64) -> Result<Option<u64>, crate::reorg::Error> {
        (**self).safe_head(observed_head).await
    }
    async fn inspect(
        &self,
        header: &crate::chain::BlockHeader,
    ) -> Result<crate::reorg::Verdict, crate::reorg::Error> {
        (**self).inspect(header).await
    }
    async fn commit(
        &self,
        header: &crate::chain::BlockHeader,
    ) -> Result<(), crate::reorg::Error> {
        (**self).commit(header).await
    }
    async fn rewind_to(
        &self,
        last_valid_block: Option<u64>,
    ) -> Result<(), crate::reorg::Error> {
        (**self).rewind_to(last_valid_block).await
    }
});

delegating_arc_impl!(crate::cursor::CursorStore {
    async fn load(
        &self,
        chain_id: u32,
    ) -> Result<Option<crate::cursor::Cursor>, crate::cursor::Error> {
        (**self).load(chain_id).await
    }
    async fn save(&self, cursor: &crate::cursor::Cursor) -> Result<(), crate::cursor::Error> {
        (**self).save(cursor).await
    }
});

delegating_arc_impl!(crate::processors::BlockProcessor {
    fn name(&self) -> &str {
        (**self).name()
    }
    async fn on_block_range(
        &self,
        from: u64,
        to: u64,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<crate::processors::Outcome, crate::processors::Failure> {
        (**self).on_block_range(from, to, cancel).await
    }
    async fn on_reorg(
        &self,
        from: u64,
        to: u64,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<crate::processors::Outcome, crate::processors::Failure> {
        (**self).on_reorg(from, to, cancel).await
    }
});
