use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use units::ChainId;

use crate::cursor::{Cursor, CursorStore, Error};
use crate::fixtures::Trace;

/// Test-only cursor table, which a case can make refuse a chosen save.
#[derive(Clone, Default)]
pub(crate) struct InMemoryCursorStore {
    state: Arc<Mutex<State>>,
    trace: Option<Trace>,
}

#[derive(Default)]
struct State {
    chains: HashMap<ChainId, Cursor>,
    saved: Vec<Cursor>,
    saves: usize,
    refuse_save: Option<usize>,
}

impl InMemoryCursorStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn traced(trace: Trace) -> Self {
        Self {
            trace: Some(trace),
            ..Self::default()
        }
    }

    pub(crate) fn holding(self, cursor: Cursor) -> Self {
        self.state().chains.insert(cursor.chain_id, cursor);

        self
    }

    pub(crate) fn refuse_nth_save(&self, nth: usize) {
        self.state().refuse_save = Some(nth);
    }

    pub(crate) fn saved(&self) -> Vec<Cursor> {
        self.state().saved.clone()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|held| held.into_inner())
    }
}

#[async_trait::async_trait]
impl CursorStore for InMemoryCursorStore {
    async fn load(&self, chain_id: ChainId) -> Result<Option<Cursor>, Error> {
        Ok(self.state().chains.get(&chain_id).copied())
    }

    async fn save(&self, cursor: &Cursor) -> Result<(), Error> {
        if let Some(trace) = &self.trace {
            trace.note(format!("cursor.save({})", cursor.last_block));
        }

        let mut state = self.state();
        state.saves += 1;
        if state.refuse_save == Some(state.saves) {
            return Err(Error::SaveRefused);
        }

        state.chains.insert(cursor.chain_id, *cursor);
        state.saved.push(*cursor);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::InMemoryCursorStore;
    use crate::cursor::conformance::{CursorHarness, cursor_store_conformance};

    struct InMemoryCursorHarness(InMemoryCursorStore);

    impl CursorHarness for InMemoryCursorHarness {
        type Store = InMemoryCursorStore;

        async fn fresh(_case: &str) -> Self {
            Self(InMemoryCursorStore::new())
        }

        fn store(&self) -> &Self::Store {
            &self.0
        }
    }

    cursor_store_conformance!(InMemoryCursorHarness);
}
