use std::cmp::Reverse;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use units::ChainId;

use crate::chain::BlockHeader;
use crate::reorg::{BlockHeaderStore, Error};

/// Test-only in-memory retention window, which a case can make refuse a chosen write.
#[derive(Clone, Default)]
pub(crate) struct InMemoryBlockHeaderStore {
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    chains: HashMap<ChainId, Vec<BlockHeader>>,
    retentions: usize,
    rewinds: usize,
    refuse_retention: Option<usize>,
    refuse_rewind: Option<usize>,
}

impl InMemoryBlockHeaderStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn refuse_nth_retention(&self, nth: usize) {
        self.state().refuse_retention = Some(nth);
    }

    pub(crate) fn refuse_nth_rewind(&self, nth: usize) {
        self.state().refuse_rewind = Some(nth);
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|held| held.into_inner())
    }
}

#[async_trait::async_trait]
impl BlockHeaderStore for InMemoryBlockHeaderStore {
    async fn load(&self, chain_id: ChainId) -> Result<Vec<BlockHeader>, Error> {
        Ok(self
            .state()
            .chains
            .get(&chain_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn append(
        &self,
        chain_id: ChainId,
        header: &BlockHeader,
        retain_from: u64,
    ) -> Result<(), Error> {
        let mut state = self.state();
        state.retentions += 1;
        if state.refuse_retention == Some(state.retentions) {
            return Err(Error::RetentionRefused);
        }

        let window = state.chains.entry(chain_id).or_default();
        window.retain(|held| held.number != header.number && held.number >= retain_from);
        window.push(*header);
        // The port promises no order; newest-first makes every case exercise the detector's sort.
        window.sort_unstable_by_key(|held| Reverse(held.number));

        Ok(())
    }

    async fn truncate(
        &self,
        chain_id: ChainId,
        last_valid_block: Option<u64>,
    ) -> Result<(), Error> {
        let mut state = self.state();
        state.rewinds += 1;
        if state.refuse_rewind == Some(state.rewinds) {
            return Err(Error::RewindRefused);
        }

        let window = state.chains.entry(chain_id).or_default();
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

    struct InMemoryBlockHeaderHarness(InMemoryBlockHeaderStore);

    impl Harness for InMemoryBlockHeaderHarness {
        type Store = InMemoryBlockHeaderStore;

        async fn fresh(_case: &str) -> Self {
            Self(InMemoryBlockHeaderStore::new())
        }

        fn store(&self) -> &Self::Store {
            &self.0
        }
    }

    block_header_store_conformance!(InMemoryBlockHeaderHarness);

    mod beyond_the_contract {
        use super::*;
        use crate::fixtures::{CHAIN_ID, header};
        use crate::reorg::BlockHeaderStore;

        #[tokio::test]
        async fn hands_back_the_window_newest_first() {
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

            assert_eq!(numbers, vec![503, 502, 501, 500]);
        }
    }
}
