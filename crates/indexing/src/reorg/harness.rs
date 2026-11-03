//! Test fixtures combining a chain_client detector_client, a block header store, and the real detector.

use std::sync::Arc;

use super::in_memory::InMemoryBlockHeaderStore;
use crate::chain::{BlockHeader, ChainClient, FakeChainClient, ForkingChainClient};
use crate::cursor::Cursor;
use crate::fixtures::{CHAIN_ID, hash_of};
use crate::reorg::{BlockHeaderStore, HashChainReorgDetector, ReorgDetector};

pub(crate) const HEAD: u64 = 1_000;

pub(crate) struct Harness {
    pub(crate) detector: HashChainReorgDetector,

    pub(crate) chain_client: Arc<FakeChainClient>,
    pub(crate) block_header_store: InMemoryBlockHeaderStore,
}

pub(crate) async fn harness(finality_depth: u64) -> Harness {
    at_head(finality_depth, HEAD).await
}

pub(crate) async fn at_head(finality_depth: u64, head: u64) -> Harness {
    let chain_client = Arc::new(FakeChainClient::at_head(head));
    let detector_client = Arc::clone(&chain_client) as Arc<dyn ChainClient>;

    assemble(finality_depth, head, chain_client, detector_client).await
}

pub(crate) fn unprimed(finality_depth: u64, head: u64) -> Harness {
    let chain_client = Arc::new(FakeChainClient::at_head(head));
    let block_header_store = InMemoryBlockHeaderStore::new();
    let detector = HashChainReorgDetector::new(
        CHAIN_ID,
        finality_depth,
        Arc::clone(&chain_client) as Arc<dyn ChainClient>,
        Box::new(block_header_store.clone()),
    );

    Harness {
        detector,
        chain_client,
        block_header_store,
    }
}

pub(crate) async fn forking(
    finality_depth: u64,
    after_reads: usize,
    fork_point: u64,
    branch: u8,
) -> Harness {
    let chain_client = Arc::new(FakeChainClient::at_head(HEAD));
    let detector_client = Arc::new(ForkingChainClient::new(
        Arc::clone(&chain_client),
        after_reads,
        fork_point,
        branch,
    )) as Arc<dyn ChainClient>;

    assemble(finality_depth, HEAD, chain_client, detector_client).await
}

async fn assemble(
    finality_depth: u64,
    head: u64,
    chain_client: Arc<FakeChainClient>,
    detector_client: Arc<dyn ChainClient>,
) -> Harness {
    let block_header_store = InMemoryBlockHeaderStore::new();
    let detector = HashChainReorgDetector::new(
        CHAIN_ID,
        finality_depth,
        detector_client,
        Box::new(block_header_store.clone()),
    );

    detector.safe_head(head).await.expect("the boundary");

    Harness {
        detector,
        chain_client,
        block_header_store,
    }
}

impl Harness {
    /// Sequential because each commit prunes relative to its own height.
    pub(crate) async fn commit(&self, blocks: impl IntoIterator<Item = u64>) {
        for block in blocks {
            self.detector
                .commit(&self.chain_client.header_at(block))
                .await
                .expect("the header should commit");
        }
    }

    pub(crate) async fn retained(&self) -> Vec<u64> {
        let mut numbers: Vec<u64> = self
            .block_header_store
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
        self.chain_client.header_at(block)
    }
}

pub(crate) fn cursor_at(last_block: u64, branch: u8) -> Cursor {
    Cursor {
        chain_id: CHAIN_ID,
        last_block,
        last_hash: hash_of(branch, last_block),
    }
}
