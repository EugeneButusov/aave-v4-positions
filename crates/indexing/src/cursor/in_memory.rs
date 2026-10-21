//! The cursor in memory. Its conformance run is below, because a double that
//! disagreed with its port would make every case using it a proof about a
//! fiction.

use std::collections::HashMap;
use std::sync::Mutex;

use units::ChainId;

use super::{Cursor, CursorStore, Error};

/// The cursor, in memory. One row per chain, overwritten in place.
#[derive(Default)]
pub(crate) struct InMemoryCursorStore {
    chains: Mutex<HashMap<ChainId, Cursor>>,
}

impl InMemoryCursorStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn chains(&self) -> std::sync::MutexGuard<'_, HashMap<ChainId, Cursor>> {
        self.chains.lock().unwrap_or_else(|held| held.into_inner())
    }
}

#[async_trait::async_trait]
impl CursorStore for InMemoryCursorStore {
    async fn load(&self, chain_id: ChainId) -> Result<Option<Cursor>, Error> {
        Ok(self.chains().get(&chain_id).copied())
    }

    async fn save(&self, cursor: &Cursor) -> Result<(), Error> {
        self.chains().insert(cursor.chain_id, *cursor);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! The port's specification, against the double that stands in for it.

    use super::InMemoryCursorStore;
    use crate::cursor::conformance::{CursorHarness, cursor_store_conformance};

    struct Double(InMemoryCursorStore);

    impl CursorHarness for Double {
        type Store = InMemoryCursorStore;

        async fn fresh(_case: &str) -> Self {
            Self(InMemoryCursorStore::new())
        }

        fn store(&self) -> &Self::Store {
            &self.0
        }
    }

    cursor_store_conformance!(Double);
}
