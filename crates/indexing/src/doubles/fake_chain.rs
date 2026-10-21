use std::sync::Mutex;

use alloy_primitives::B256;
use async_trait::async_trait;

use crate::chain::{BlockHeader, ChainClient, Error};
use crate::fixtures::{hash_of, timestamp_of};

/// What was asked of the chain, so a case can assert on what was **not**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChainCall {
    ChainId,
    Head,
    Header(u64),
}

/// One lock over everything, rather than one per field: a case that reads the
/// call log after setting a fork wants both to describe the same moment.
struct State {
    chain_id: u32,
    head: u64,

    /// `(above, branch)` in the order declared, resolved last-match-wins, so
    /// declaring a fork rewrites every block above its point at once.
    forks: Vec<(u64, u8)>,
    failures: usize,
    calls: Vec<ChainCall>,
}

/// A chain whose ancestry is **derived rather than stored**.
///
/// A block's hash is its branch and its height, so a fork needs no rewriting of
/// anything: the moment one is declared, every header above its point is served
/// from the new branch and is self-consistent with its neighbours.
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

    /// Poisoning is ignored: a case that panicked holding this has failed, and
    /// what it left behind is the evidence.
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|held| held.into_inner())
    }

    pub(crate) fn set_head(&self, head: u64) {
        self.state().head = head;
    }

    pub(crate) fn set_chain_id(&self, chain_id: u32) {
        self.state().chain_id = chain_id;
    }

    /// Everything strictly above `above` now belongs to `branch`.
    pub(crate) fn fork_above(&self, above: u64, branch: u8) {
        self.state().forks.push((above, branch));
    }

    /// The next `count` calls of any kind refuse.
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

    /// Which branch `block` sits on. Later declarations win, so a second fork
    /// over the first is the chain changing its mind twice.
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

    /// The header this chain serves for `block`, composed one side at a time:
    /// the hash from this block's branch, the parent from the branch of the
    /// block below. They differ at exactly one height — the fork point — which
    /// is what makes a fork findable at all.
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
    async fn chain_id(&self) -> Result<u32, Error> {
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
