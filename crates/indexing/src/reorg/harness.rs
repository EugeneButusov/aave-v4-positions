//! Test fixtures combining a chain, a window, and the real detector.

use std::sync::Arc;

use super::in_memory::InMemoryBlockHeaderStore;
use crate::chain::{BlockHeader, ChainClient, FakeChainClient, ForkingChainClient};
use crate::cursor::Cursor;
use crate::fixtures::{CHAIN_ID, hash_of};
use crate::reorg::{BlockHeaderStore, HashChainReorgDetector, ReorgDetector};

pub(crate) const HEAD: u64 = 1_000;

pub(crate) struct Harness<W = InMemoryBlockHeaderStore> {
    pub(crate) detector: HashChainReorgDetector,

    pub(crate) chain: Arc<FakeChainClient>,
    pub(crate) window: W,
}

pub(crate) async fn harness(finality_depth: u64) -> Harness {
    over(finality_depth, HEAD, InMemoryBlockHeaderStore::new()).await
}

pub(crate) async fn over<W>(finality_depth: u64, head: u64, window: W) -> Harness<W>
where
    W: BlockHeaderStore + Clone + 'static,
{
    let chain = Arc::new(FakeChainClient::at_head(head));
    let client = Arc::clone(&chain) as Arc<dyn ChainClient>;

    assemble(finality_depth, head, chain, client, window).await
}

pub(crate) fn unprimed(finality_depth: u64, head: u64) -> Harness {
    let chain = Arc::new(FakeChainClient::at_head(head));
    let window = InMemoryBlockHeaderStore::new();
    let detector = HashChainReorgDetector::new(
        CHAIN_ID,
        finality_depth,
        Arc::clone(&chain) as Arc<dyn ChainClient>,
        Box::new(window.clone()),
    );

    Harness {
        detector,
        chain,
        window,
    }
}

pub(crate) async fn forking(
    finality_depth: u64,
    after_reads: usize,
    fork_point: u64,
    branch: u8,
) -> Harness {
    let chain = Arc::new(FakeChainClient::at_head(HEAD));
    let client = Arc::new(ForkingChainClient::new(
        Arc::clone(&chain),
        after_reads,
        fork_point,
        branch,
    )) as Arc<dyn ChainClient>;

    assemble(
        finality_depth,
        HEAD,
        chain,
        client,
        InMemoryBlockHeaderStore::new(),
    )
    .await
}

async fn assemble<W>(
    finality_depth: u64,
    head: u64,
    chain: Arc<FakeChainClient>,
    client: Arc<dyn ChainClient>,
    window: W,
) -> Harness<W>
where
    W: BlockHeaderStore + Clone + 'static,
{
    let detector =
        HashChainReorgDetector::new(CHAIN_ID, finality_depth, client, Box::new(window.clone()));

    detector.safe_head(head).await.expect("the boundary");

    Harness {
        detector,
        chain,
        window,
    }
}

impl<W: BlockHeaderStore> Harness<W> {
    /// Sequential because each commit prunes relative to its own height.
    pub(crate) async fn commit(&self, blocks: impl IntoIterator<Item = u64>) {
        for block in blocks {
            self.detector
                .commit(&self.chain.header_at(block))
                .await
                .expect("the header should commit");
        }
    }

    pub(crate) async fn retained(&self) -> Vec<u64> {
        let mut numbers: Vec<u64> = self
            .window
            .load(CHAIN_ID)
            .await
            .expect("the window should read")
            .iter()
            .map(|held| held.number)
            .collect();
        numbers.sort_unstable();

        numbers
    }

    pub(crate) fn header(&self, block: u64) -> BlockHeader {
        self.chain.header_at(block)
    }
}

pub(crate) fn cursor_at(last_block: u64, branch: u8) -> Cursor {
    Cursor {
        chain_id: CHAIN_ID,
        last_block,
        last_hash: hash_of(branch, last_block),
    }
}
