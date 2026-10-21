use std::collections::HashMap;
use std::sync::Mutex;

use crate::cursor::{Cursor, CursorStore, Error};
use crate::doubles::detector::{Trace, note};

/// The cursor, in memory. One row per chain, overwritten in place.
#[derive(Default)]
pub(crate) struct InMemoryCursorStore {
    chains: Mutex<HashMap<u32, Cursor>>,
}

impl InMemoryCursorStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn chains(&self) -> std::sync::MutexGuard<'_, HashMap<u32, Cursor>> {
        self.chains.lock().unwrap_or_else(|held| held.into_inner())
    }
}

#[async_trait::async_trait]
impl CursorStore for InMemoryCursorStore {
    async fn load(&self, chain_id: u32) -> Result<Option<Cursor>, Error> {
        Ok(self.chains().get(&chain_id).copied())
    }

    async fn save(&self, cursor: &Cursor) -> Result<(), Error> {
        self.chains().insert(cursor.chain_id, *cursor);

        Ok(())
    }
}

/// What the loop saved, and a way to make one save refuse.
///
/// **How "the cursor is the commit point" is proven**: if the save rejects, the
/// same range has to be dispatched again. A store that could not refuse would
/// leave that clause asserted by argument alone.
#[derive(Default)]
pub(crate) struct RecordingCursorStore {
    inner: InMemoryCursorStore,
    log: Mutex<Log>,

    /// Shared with the detector and the processors when a case cares about the
    /// order the three were written in — which is the whole of "the cursor is
    /// the commit point".
    trace: Option<Trace>,
}

#[derive(Default)]
struct Log {
    saved: Vec<Cursor>,
    saves: usize,
    fail_save: Option<usize>,
}

impl RecordingCursorStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn traced(trace: Trace) -> Self {
        Self {
            trace: Some(trace),
            ..Self::default()
        }
    }

    /// Already holding a position, which is what a resume reads.
    pub(crate) fn traced_holding(cursor: Cursor, trace: Trace) -> Self {
        let store = Self::traced(trace);
        store.inner.chains().insert(cursor.chain_id, cursor);

        store
    }

    fn log(&self) -> std::sync::MutexGuard<'_, Log> {
        self.log.lock().unwrap_or_else(|held| held.into_inner())
    }

    /// 1-based.
    pub(crate) fn fail_save(&self, nth: usize) {
        self.log().fail_save = Some(nth);
    }

    pub(crate) fn saved(&self) -> Vec<Cursor> {
        self.log().saved.clone()
    }
}

#[async_trait::async_trait]
impl CursorStore for RecordingCursorStore {
    async fn load(&self, chain_id: u32) -> Result<Option<Cursor>, Error> {
        self.inner.load(chain_id).await
    }

    async fn save(&self, cursor: &Cursor) -> Result<(), Error> {
        if let Some(trace) = &self.trace {
            note(trace, format!("cursor.save({})", cursor.last_block));
        }

        {
            let mut log = self.log();
            log.saves += 1;
            // Refused before anything is recorded, so a rejected save leaves the
            // loop pointing at the last position actually persisted.
            if log.fail_save == Some(log.saves) {
                return Err(Error::unavailable("the cursor store is unavailable"));
            }
        }

        self.inner.save(cursor).await?;
        self.log().saved.push(*cursor);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! The port's specification, against the doubles that stand in for it.

    use super::{InMemoryCursorStore, RecordingCursorStore};
    use crate::cursor::conformance::{CursorHarness, cursor_store_conformance};

    struct Plain(InMemoryCursorStore);

    impl CursorHarness for Plain {
        type Store = InMemoryCursorStore;

        async fn fresh(_case: &str) -> Self {
            Self(InMemoryCursorStore::new())
        }

        fn store(&self) -> &Self::Store {
            &self.0
        }
    }

    cursor_store_conformance!(Plain);

    mod recording {
        use super::*;

        struct Recording(RecordingCursorStore);

        impl CursorHarness for Recording {
            type Store = RecordingCursorStore;

            async fn fresh(_case: &str) -> Self {
                Self(RecordingCursorStore::new())
            }

            fn store(&self) -> &Self::Store {
                &self.0
            }
        }

        // The recording wrapper is a store too, so it answers for the port
        // rather than being trusted to pass writes through.
        cursor_store_conformance!(Recording);
    }
}
