//! What the loop is configured with and wired to.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use units::ChainId;

use crate::chain::ChainClient;
use crate::cursor::CursorStore;
use crate::engine::metrics::Instruments;
use crate::engine::status::IndexerStatus;
use crate::processors::BlockProcessor;
use crate::reorg::ReorgDetector;

/// What the loop reads, and nothing more: the finality depth is the detector's,
/// the provider list the chain client's, the stall threshold the readiness check's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub chain_id: ChainId,

    /// The floor, used only when the cursor store has nothing.
    pub start_block: u64,

    /// Upper bound on one dispatched range, halved at runtime if a processor
    /// asks.
    pub max_range_size: u64,

    /// How long to wait after catching up.
    pub poll_interval: Duration,
}

/// Everything the loop is wired to, named at the call site.
///
/// A struct rather than eight positional arguments, which is the shape
/// `bins/api` composes its router from and for the same reason: the collaborator
/// list is long enough that the order stops being self-evident.
pub struct Pipeline {
    pub options: Options,

    /// Shared, because the detector reads the head through the same client and
    /// two of them over one provider list diverge the moment one fails over.
    pub chain_client: Arc<dyn ChainClient>,
    pub reorg_detector: Box<dyn ReorgDetector>,
    pub cursor_store: Box<dyn CursorStore>,

    /// Dispatched in this order, stopping at the first that does not accept a
    /// range.
    pub processors: Vec<Box<dyn BlockProcessor>>,

    /// Shared with the gauges and the readiness check.
    pub status: Arc<IndexerStatus>,
    pub instruments: Instruments,
    pub cancel: CancellationToken,
}
