use std::collections::HashMap;
use std::sync::Mutex;

use units::ChainId;

use crate::cursor::{Cursor, CursorStore, Error};

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
