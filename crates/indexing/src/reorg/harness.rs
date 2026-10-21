//! A chain, a window and the real detector over both.
//!
//! A fixture for this module's own cases rather than anything a consumer could
//! hold: it builds one concrete detector.

use std::sync::Arc;

use crate::chain::{BlockHeader, ChainClient};
use crate::cursor::Cursor;
use crate::doubles::{FakeChain, ForkingChain, InMemoryBlockHeaderStore};
use crate::fixtures::{CHAIN_ID, hash_of};
use crate::reorg::{BlockHeaderStore, HashChainReorgDetector, ReorgDetector};

/// Far enough above any depth a case uses that the tip is never genesis.
pub(crate) const HEAD: u64 = 1_000;

pub(crate) struct Harness<W = InMemoryBlockHeaderStore> {
    pub(crate) detector: HashChainReorgDetector,

    /// The chain the detector reads, or the one a [`ForkingChain`] wraps — a
    /// case declares forks and counts calls on this either way.
    pub(crate) chain: Arc<FakeChain>,
    pub(crate) window: Arc<W>,
}

/// The default: a plain chain at [`HEAD`] and an in-memory window.
pub(crate) async fn harness(finality_depth: u64) -> Harness {
    over(
        finality_depth,
        HEAD,
        Arc::new(InMemoryBlockHeaderStore::new()),
    )
    .await
}

/// A window of the caller's choosing, so a store that refuses or reorders can
/// stand behind the same detector.
pub(crate) async fn over<W>(finality_depth: u64, head: u64, window: Arc<W>) -> Harness<W>
where
    W: BlockHeaderStore + 'static,
{
    let chain = Arc::new(FakeChain::at_head(head));
    let client = Arc::clone(&chain) as Arc<dyn ChainClient>;

    assemble(finality_depth, head, chain, client, window).await
}

/// Without the head primed, which is the state `bootstrap` really runs in: it
/// is called before the loop has observed one.
pub(crate) fn unprimed(finality_depth: u64, head: u64) -> Harness {
    let chain = Arc::new(FakeChain::at_head(head));
    let window = Arc::new(InMemoryBlockHeaderStore::new());
    let detector = HashChainReorgDetector::new(
        CHAIN_ID,
        finality_depth,
        Arc::clone(&chain) as Arc<dyn ChainClient>,
        Box::new(Arc::clone(&window)),
    );

    Harness {
        detector,
        chain,
        window,
    }
}

/// A chain that forks after `after_reads` header reads, which is the one shape
/// a plain chain cannot script.
pub(crate) async fn forking(
    finality_depth: u64,
    after_reads: usize,
    fork_point: u64,
    branch: u8,
) -> Harness {
    let chain = Arc::new(FakeChain::at_head(HEAD));
    let client = Arc::new(ForkingChain::new(
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
        Arc::new(InMemoryBlockHeaderStore::new()),
    )
    .await
}

/// **Primes the boundary before anything else is asked.** The detector caches
/// the head it was last shown, and a case that skipped this would have its first
/// call spend a header read and then measure a different number of them.
async fn assemble<W>(
    finality_depth: u64,
    head: u64,
    chain: Arc<FakeChain>,
    client: Arc<dyn ChainClient>,
    window: Arc<W>,
) -> Harness<W>
where
    W: BlockHeaderStore + 'static,
{
    let detector = HashChainReorgDetector::new(
        CHAIN_ID,
        finality_depth,
        client,
        Box::new(Arc::clone(&window)),
    );

    detector.safe_head(head).await.expect("the boundary");

    Harness {
        detector,
        chain,
        window,
    }
}

impl<W: BlockHeaderStore> Harness<W> {
    /// Sequentially on purpose: each commit prunes relative to its own height,
    /// so the order they land in is the retention behaviour under test.
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

/// A cursor at `last_block` on `branch`.
pub(crate) fn cursor_at(last_block: u64, branch: u8) -> Cursor {
    Cursor {
        chain_id: CHAIN_ID,
        last_block,
        last_hash: hash_of(branch, last_block),
    }
}
