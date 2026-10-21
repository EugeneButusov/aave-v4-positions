use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use crate::chain::{BlockHeader, ChainClient, Error};
use crate::doubles::FakeChain;

/// A chain that forks **part way through a walk**, which is the one thing
/// [`FakeChain`] cannot script on its own: its forks apply the moment they are
/// declared, so every header it then serves is already self-consistent.
///
/// Counts header reads only, because that is what a backwards walk spends.
pub(crate) struct ForkingChain {
    inner: Arc<FakeChain>,
    after_reads: usize,
    fork_point: u64,
    branch: u8,
    reads: AtomicUsize,
}

impl ForkingChain {
    pub(crate) fn new(
        inner: Arc<FakeChain>,
        after_reads: usize,
        fork_point: u64,
        branch: u8,
    ) -> Self {
        Self {
            inner,
            after_reads,
            fork_point,
            branch,
            reads: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl ChainClient for ForkingChain {
    async fn chain_id(&self) -> Result<u32, Error> {
        self.inner.chain_id().await
    }

    async fn head_block_number(&self) -> Result<u64, Error> {
        self.inner.head_block_number().await
    }

    async fn block_header(&self, number: u64) -> Result<BlockHeader, Error> {
        if self.reads.fetch_add(1, Ordering::Relaxed) == self.after_reads {
            self.inner.fork_above(self.fork_point, self.branch);
        }

        self.inner.block_header(number).await
    }
}
