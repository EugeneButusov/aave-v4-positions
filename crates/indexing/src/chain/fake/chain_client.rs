use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use alloy_primitives::B256;
use async_trait::async_trait;
use units::ChainId;

use crate::chain::{BlockHeader, ChainClient, Error};
use crate::fixtures::{hash_of, timestamp_of};

/// Calls made through the chain port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChainCall {
    ChainId,
    Head,
    Header(u64),
}

struct State {
    chain_id: ChainId,
    head: u64,

    forks: Vec<(u64, u8)>,
    failures: usize,
    calls: Vec<ChainCall>,
}

/// Derives each header from its branch and height, making forks self-consistent.
pub(crate) struct FakeChain {
    state: Mutex<State>,
}

impl FakeChain {
    pub(crate) fn at_head(head: u64) -> Self {
        Self {
            state: Mutex::new(State {
                chain_id: crate::fixtures::CHAIN_ID,
                head,
                forks: Vec::new(),
                failures: 0,
                calls: Vec::new(),
            }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|held| held.into_inner())
    }

    pub(crate) fn fork_above(&self, above: u64, branch: u8) {
        self.state().forks.push((above, branch));
    }

    pub(crate) fn fail_next(&self, count: usize) {
        self.state().failures = count;
    }

    pub(crate) fn calls(&self) -> Vec<ChainCall> {
        self.state().calls.clone()
    }

    pub(crate) fn header_reads(&self) -> usize {
        self.calls()
            .iter()
            .filter(|call| matches!(call, ChainCall::Header(_)))
            .count()
    }

    pub(crate) fn branch_at(&self, block: u64) -> u8 {
        let state = self.state();

        state
            .forks
            .iter()
            .filter(|(above, _)| block > *above)
            .map(|(_, branch)| *branch)
            .next_back()
            .unwrap_or(b'a')
    }

    /// A fork shares the block below it, so the parent is taken from that
    /// block's own branch rather than from the forked one.
    pub(crate) fn header_at(&self, block: u64) -> BlockHeader {
        BlockHeader {
            number: block,
            hash: hash_of(self.branch_at(block), block),
            parent_hash: block
                .checked_sub(1)
                .map_or(B256::ZERO, |below| hash_of(self.branch_at(below), below)),
            timestamp: timestamp_of(block),
        }
    }

    fn record(&self, call: ChainCall) -> Result<(), Error> {
        let mut state = self.state();
        state.calls.push(call);

        if state.failures > 0 {
            state.failures -= 1;

            return Err(Error::Unavailable {
                source: "rpc unavailable".into(),
            });
        }

        Ok(())
    }
}

#[async_trait]
impl ChainClient for FakeChain {
    async fn chain_id(&self) -> Result<ChainId, Error> {
        self.record(ChainCall::ChainId)?;

        Ok(self.state().chain_id)
    }

    async fn head_block_number(&self) -> Result<u64, Error> {
        self.record(ChainCall::Head)?;

        Ok(self.state().head)
    }

    async fn block_header(&self, number: u64) -> Result<BlockHeader, Error> {
        self.record(ChainCall::Header(number))?;

        Ok(self.header_at(number))
    }
}

/// Applies a fork after a configured number of header reads.
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
    async fn chain_id(&self) -> Result<ChainId, Error> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_every_header_to_the_one_below_it_across_a_fork() {
        let chain = FakeChain::at_head(100);
        chain.fork_above(50, b'b');

        for block in 49..=52 {
            assert_eq!(
                chain.header_at(block + 1).parent_hash,
                chain.header_at(block).hash,
                "block {} does not extend {block}",
                block + 1,
            );
        }
    }
}
