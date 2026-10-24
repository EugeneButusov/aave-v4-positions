//! The retention window in memory. Its conformance run is below, because a
//! double that disagreed with its port would make every case using it a proof
//! about a fiction.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use units::ChainId;

use super::{BlockHeaderStore, Error};
use crate::chain::BlockHeader;

/// So a case can keep the store it also handed to the detector, which owns the
/// port as a `Box<dyn _>` and leaves nothing to read the window through.
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

/// What the detector wrote, and a way to make one write refuse.
///
/// Recording happens **after** the write succeeded, so what it holds is what the
/// window actually took.
#[derive(Default)]
pub(crate) struct RecordingBlockHeaderStore {
    inner: InMemoryBlockHeaderStore,
    log: Mutex<Log>,
}

#[derive(Default)]
struct Log {
    appended: Vec<BlockHeader>,
    truncated_at: Vec<Option<u64>>,
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

    /// 1-based, and counted separately from rewinds.
    pub(crate) fn refuse_retention(&self, nth: usize) {
        self.log().refuse_retention = Some(nth);
    }

    pub(crate) fn refuse_rewind(&self, nth: usize) {
        self.log().refuse_rewind = Some(nth);
    }

    pub(crate) fn appended(&self) -> Vec<BlockHeader> {
        self.log().appended.clone()
    }

    pub(crate) fn truncated_at(&self) -> Vec<Option<u64>> {
        self.log().truncated_at.clone()
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
            // Refused before anything is mutated: a store that failed having
            // already written would make the detector's "surfaces rather than
            // reporting a header it does not hold" untestable.
            if log.refuse_retention == Some(log.retentions) {
                return Err(Error::RetentionRefused);
            }
        }

        self.inner.append(chain_id, header, retain_from).await?;
        self.log().appended.push(*header);

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
        self.log().truncated_at.push(last_valid_block);

        Ok(())
    }
}

/// Hands the window back newest-first.
///
/// Stands in for an adapter that reads rows without an `ORDER BY`, which the
/// port permits — so this is what proves the detector sorts what it reads
/// rather than leaning on an order nothing promised.
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

/// The retention window, in memory. **A test double and nothing else**, for the
/// reason [`BlockHeaderStore`] gives.
#[derive(Default)]
pub(crate) struct InMemoryBlockHeaderStore {
    /// **A sorted list per chain, not a map keyed by height.** Re-inserting a
    /// key keeps its original position, so a block re-committed on another
    /// branch would sit in the wrong place and the ancestor walk would read one
    /// branch as a continuation of the other.
    chains: Mutex<HashMap<ChainId, Vec<BlockHeader>>>,
}

impl InMemoryBlockHeaderStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Poisoning is ignored: a case that panicked holding this lock has
    /// already failed.
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
    //! The port's specification, against the double that stands in for it.

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

        /// An implementation detail, pinned because the detector's own cases
        /// read this double's window positionally — and `load` promises no
        /// order, which is why it is not in the suite.
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
