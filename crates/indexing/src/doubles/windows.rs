use std::sync::Mutex;

use async_trait::async_trait;

use crate::chain::BlockHeader;
use crate::doubles::InMemoryBlockHeaderStore;
use crate::reorg::{BlockHeaderStore, Error};

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
    appends: usize,
    truncates: usize,
    fail_append: Option<usize>,
    fail_truncate: Option<usize>,
}

impl RecordingBlockHeaderStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn log(&self) -> std::sync::MutexGuard<'_, Log> {
        self.log.lock().unwrap_or_else(|held| held.into_inner())
    }

    /// 1-based, and counted separately from truncates.
    pub(crate) fn fail_append(&self, nth: usize) {
        self.log().fail_append = Some(nth);
    }

    pub(crate) fn fail_truncate(&self, nth: usize) {
        self.log().fail_truncate = Some(nth);
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
    async fn load(&self, chain_id: u32) -> Result<Vec<BlockHeader>, Error> {
        self.inner.load(chain_id).await
    }

    async fn append(
        &self,
        chain_id: u32,
        header: &BlockHeader,
        retain_from: u64,
    ) -> Result<(), Error> {
        {
            let mut log = self.log();
            log.appends += 1;
            // Refused before anything is mutated: a store that failed having
            // already written would make the detector's "surfaces rather than
            // reporting a header it does not hold" untestable.
            if log.fail_append == Some(log.appends) {
                return Err(Error::unavailable("the header store is unavailable"));
            }
        }

        self.inner.append(chain_id, header, retain_from).await?;
        self.log().appended.push(*header);

        Ok(())
    }

    async fn truncate(&self, chain_id: u32, last_valid_block: Option<u64>) -> Result<(), Error> {
        {
            let mut log = self.log();
            log.truncates += 1;
            if log.fail_truncate == Some(log.truncates) {
                return Err(Error::unavailable("the header store is unavailable"));
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
    async fn load(&self, chain_id: u32) -> Result<Vec<BlockHeader>, Error> {
        let mut window = self.inner.load(chain_id).await?;
        window.reverse();

        Ok(window)
    }

    async fn append(
        &self,
        chain_id: u32,
        header: &BlockHeader,
        retain_from: u64,
    ) -> Result<(), Error> {
        self.inner.append(chain_id, header, retain_from).await
    }

    async fn truncate(&self, chain_id: u32, last_valid_block: Option<u64>) -> Result<(), Error> {
        self.inner.truncate(chain_id, last_valid_block).await
    }
}
