use std::collections::HashMap;
use std::sync::Mutex;

use units::ChainId;

use crate::chain::BlockHeader;
use crate::reorg::{BlockHeaderStore, Error};

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
