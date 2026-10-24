//! In-memory and instrumented implementations of the reorg window port.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use units::ChainId;

use super::{BlockHeaderStore, Error};
use crate::chain::BlockHeader;

#[async_trait]
impl<S: BlockHeaderStore + ?Sized> BlockHeaderStore for Arc<S> {
    async fn load(&self, chain_id: ChainId) -> Result<Vec<BlockHeader>, Error> {
        (**self).load(chain_id).await
    }

    async fn append(
        &self,
        chain_id: ChainId,
        header: &BlockHeader,
        retain_from: u64,
    ) -> Result<(), Error> {
        (**self).append(chain_id, header, retain_from).await
    }

    async fn truncate(
        &self,
        chain_id: ChainId,
        last_valid_block: Option<u64>,
    ) -> Result<(), Error> {
        (**self).truncate(chain_id, last_valid_block).await
    }
}

/// Records successful transitions and can refuse a selected transition.
#[derive(Default)]
pub(crate) struct RecordingBlockHeaderStore {
    inner: InMemoryBlockHeaderStore,
    log: Mutex<Log>,
}

#[derive(Default)]
struct Log {
    retained: Vec<BlockHeader>,
    rewound_to: Vec<Option<u64>>,
    retentions: usize,
    rewinds: usize,
    refuse_retention: Option<usize>,
    refuse_rewind: Option<usize>,
}

impl RecordingBlockHeaderStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn log(&self) -> std::sync::MutexGuard<'_, Log> {
        self.log.lock().unwrap_or_else(|held| held.into_inner())
    }

    pub(crate) fn refuse_nth_retention(&self, nth: usize) {
        self.log().refuse_retention = Some(nth);
    }

    pub(crate) fn refuse_nth_rewind(&self, nth: usize) {
        self.log().refuse_rewind = Some(nth);
    }

    pub(crate) fn retained(&self) -> Vec<BlockHeader> {
        self.log().retained.clone()
    }

    pub(crate) fn rewound_to(&self) -> Vec<Option<u64>> {
        self.log().rewound_to.clone()
    }
}

#[async_trait]
impl BlockHeaderStore for RecordingBlockHeaderStore {
    async fn load(&self, chain_id: ChainId) -> Result<Vec<BlockHeader>, Error> {
        self.inner.load(chain_id).await
    }

    async fn append(
        &self,
        chain_id: ChainId,
        header: &BlockHeader,
        retain_from: u64,
    ) -> Result<(), Error> {
        {
            let mut log = self.log();
            log.retentions += 1;
            // Refuse before mutation so the log describes completed transitions.
            if log.refuse_retention == Some(log.retentions) {
                return Err(Error::RetentionRefused);
            }
        }

        self.inner.append(chain_id, header, retain_from).await?;
        self.log().retained.push(*header);

        Ok(())
    }

    async fn truncate(
        &self,
        chain_id: ChainId,
        last_valid_block: Option<u64>,
    ) -> Result<(), Error> {
        {
            let mut log = self.log();
            log.rewinds += 1;
            if log.refuse_rewind == Some(log.rewinds) {
                return Err(Error::RewindRefused);
            }
        }

        self.inner.truncate(chain_id, last_valid_block).await?;
        self.log().rewound_to.push(last_valid_block);

        Ok(())
    }
}

/// Returns the window newest-first to exercise the port's unspecified order.
#[derive(Default)]
pub(crate) struct ShufflingBlockHeaderStore {
    inner: InMemoryBlockHeaderStore,
}

impl ShufflingBlockHeaderStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl BlockHeaderStore for ShufflingBlockHeaderStore {
    async fn load(&self, chain_id: ChainId) -> Result<Vec<BlockHeader>, Error> {
        let mut window = self.inner.load(chain_id).await?;
        window.reverse();

        Ok(window)
    }

    async fn append(
        &self,
        chain_id: ChainId,
        header: &BlockHeader,
        retain_from: u64,
    ) -> Result<(), Error> {
        self.inner.append(chain_id, header, retain_from).await
    }

    async fn truncate(
        &self,
        chain_id: ChainId,
        last_valid_block: Option<u64>,
    ) -> Result<(), Error> {
        self.inner.truncate(chain_id, last_valid_block).await
    }
}

/// Test-only in-memory retention window.
#[derive(Default)]
pub(crate) struct InMemoryBlockHeaderStore {
    chains: Mutex<HashMap<ChainId, Vec<BlockHeader>>>,
}

impl InMemoryBlockHeaderStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn chains(&self) -> std::sync::MutexGuard<'_, HashMap<ChainId, Vec<BlockHeader>>> {
        self.chains.lock().unwrap_or_else(|held| held.into_inner())
    }
}

#[async_trait::async_trait]
impl BlockHeaderStore for InMemoryBlockHeaderStore {
    async fn load(&self, chain_id: ChainId) -> Result<Vec<BlockHeader>, Error> {
        Ok(self.chains().get(&chain_id).cloned().unwrap_or_default())
    }

    async fn append(
        &self,
        chain_id: ChainId,
        header: &BlockHeader,
        retain_from: u64,
    ) -> Result<(), Error> {
        let mut chains = self.chains();
        let window = chains.entry(chain_id).or_default();

        window.retain(|held| held.number != header.number && held.number >= retain_from);
        window.push(*header);
        window.sort_unstable_by_key(|held| held.number);

        Ok(())
    }

    async fn truncate(
        &self,
        chain_id: ChainId,
        last_valid_block: Option<u64>,
    ) -> Result<(), Error> {
        let mut chains = self.chains();
        let window = chains.entry(chain_id).or_default();

        match last_valid_block {
            Some(block) => window.retain(|held| held.number <= block),
            None => window.clear(),
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! The port contract against its in-memory implementation.

    use super::InMemoryBlockHeaderStore;
    use crate::reorg::conformance::{Harness, block_header_store_conformance};

    struct Double(InMemoryBlockHeaderStore);

    impl Harness for Double {
        type Store = InMemoryBlockHeaderStore;

        async fn fresh(_case: &str) -> Self {
            Self(InMemoryBlockHeaderStore::new())
        }

        fn store(&self) -> &Self::Store {
            &self.0
        }
    }

    block_header_store_conformance!(Double);

    mod beyond_the_contract {
        use super::*;
        use crate::fixtures::{CHAIN_ID, header};
        use crate::reorg::BlockHeaderStore;

        #[tokio::test]
        async fn hands_back_a_window_in_ascending_order() {
            let store = InMemoryBlockHeaderStore::new();

            for block in [503_u64, 500, 502, 501] {
                store
                    .append(CHAIN_ID, &header(block, b'a'), 0)
                    .await
                    .unwrap();
            }

            let numbers: Vec<u64> = store
                .load(CHAIN_ID)
                .await
                .unwrap()
                .iter()
                .map(|header| header.number)
                .collect();

            assert_eq!(numbers, vec![500, 501, 502, 503]);
        }
    }
}
