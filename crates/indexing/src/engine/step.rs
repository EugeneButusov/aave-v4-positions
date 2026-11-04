//! What one iteration does: ask the chain, ask the detector, dispatch, record.

use std::sync::Arc;

use alloy_primitives::B256;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::chain::{BlockHeader, ChainClient};
use crate::cursor::{Cursor, CursorStore};
use crate::engine::cause::describe;
use crate::engine::dispatch::{self, Dispatch, Operation};
use crate::engine::metrics::{Instruments, Phase};
use crate::engine::pipeline::Pipeline;
use crate::engine::status::IndexerStatus;
use crate::engine::{Iteration, Options};
use crate::processors::BlockProcessor;
use crate::reorg::{ReorgDetector, Verdict};

/// Knows block numbers and nothing else: finality and the shape of a fork are
/// the detector's answers. At or below its safe head blocks go out in ranges,
/// above it one at a time, and a range never straddles the two.
pub(crate) struct Step {
    options: Options,
    chain_client: Arc<dyn ChainClient>,
    reorg_detector: Box<dyn ReorgDetector>,
    cursor_store: Box<dyn CursorStore>,
    processors: Vec<Box<dyn BlockProcessor>>,
    status: Arc<IndexerStatus>,
    instruments: Instruments,
    cancel: CancellationToken,

    cursor: Option<Cursor>,
    chain_id_verified: bool,
    bootstrapped: bool,
    effective_max_range: u64,
}

impl Step {
    pub(crate) fn new(pipeline: Pipeline) -> Self {
        let Pipeline {
            options,
            chain_client,
            reorg_detector,
            cursor_store,
            processors,
            status,
            instruments,
            cancel,
        } = pipeline;

        Self {
            effective_max_range: options.max_range_size,
            cursor: None,
            chain_id_verified: false,
            bootstrapped: false,
            options,
            chain_client,
            reorg_detector,
            cursor_store,
            processors,
            status,
            instruments,
            cancel,
        }
    }

    pub(crate) async fn advance(&mut self) -> Iteration {
        match self.attempt().await {
            Ok(result) => result,
            // Never terminal: a defect keeps failing here and surfaces through the stall signal.
            Err(reason) => Iteration::Retry {
                reason: format!("unhandled: {reason}"),
            },
        }
    }

    async fn attempt(&mut self) -> Result<Iteration, String> {
        if let Some(decided) = self.verify_chain_id().await? {
            return Ok(decided);
        }

        if let Some(decided) = self.bootstrap().await? {
            return Ok(decided);
        }

        // The status clamps it: a failover can otherwise move the head backwards.
        let head = self.status.observe_head(
            self.chain_client
                .head_block_number()
                .await
                .map_err(describe)?,
        );

        let next = self
            .cursor
            .map_or(self.options.start_block, |cursor| cursor.last_block + 1);

        if next > head {
            return Ok(Iteration::Idle);
        }

        let safe_head = self.reorg_detector.safe_head(head);
        let to = match safe_head {
            Some(settled) if next <= settled => (next + self.effective_max_range - 1).min(settled),
            _ => next,
        };

        self.index_range(next, to, safe_head).await
    }

    /// Lazily, not at construction: a boot that needs a node turns an outage into a crash loop.
    async fn verify_chain_id(&mut self) -> Result<Option<Iteration>, String> {
        if self.chain_id_verified {
            return Ok(None);
        }

        let actual = self.chain_client.chain_id().await.map_err(describe)?;

        if actual != self.options.chain_id {
            return Ok(Some(Iteration::Failed {
                reason: format!(
                    "chain id mismatch: configured {}, provider reports {actual}",
                    self.options.chain_id
                ),
            }));
        }

        self.chain_id_verified = true;

        Ok(None)
    }

    /// Set as done only once it succeeds, so a transient failure re-runs the whole of it.
    async fn bootstrap(&mut self) -> Result<Option<Iteration>, String> {
        if self.bootstrapped {
            return Ok(None);
        }

        let stored = self
            .cursor_store
            .load(self.options.chain_id)
            .await
            .map_err(describe)?;
        let verdict = self
            .reorg_detector
            .bootstrap(stored.as_ref())
            .await
            .map_err(describe)?;

        match verdict {
            Verdict::Unrecoverable { reason } => Ok(Some(Iteration::Failed {
                reason: format!("cannot resume: {reason}"),
            })),
            Verdict::Reorg {
                first_invalid_block,
                last_invalid_block,
                last_valid_hash,
            } => {
                let rewound = self
                    .unwind(
                        first_invalid_block,
                        last_invalid_block,
                        last_valid_hash,
                        Phase::Bootstrap,
                        None,
                    )
                    .await?;
                self.bootstrapped = matches!(rewound, Iteration::Progressed { .. });

                Ok(Some(rewound))
            }
            Verdict::Continuous => {
                self.cursor = stored;
                self.bootstrapped = true;

                match stored {
                    Some(cursor) => info!(
                        last_block = cursor.last_block,
                        "resuming from the stored cursor"
                    ),
                    None => info!(
                        start_block = self.options.start_block,
                        "no stored cursor; starting fresh"
                    ),
                }

                Ok(None)
            }
        }
    }

    /// The top header is read even for a settled range, so `last_hash` can always anchor a resume.
    async fn index_range(
        &mut self,
        from: u64,
        to: u64,
        safe_head: Option<u64>,
    ) -> Result<Iteration, String> {
        let header = self.chain_client.block_header(to).await.map_err(describe)?;

        if let Some(decided) = self.verify_ancestry(&header, safe_head).await? {
            return Ok(decided);
        }

        if let Some(refusal) = self.dispatch(Operation::BlockRange, from, to).await {
            return Ok(refusal);
        }

        self.reorg_detector
            .commit(&header, safe_head)
            .await
            .map_err(describe)?;
        self.save_cursor(header.number, header.hash).await?;

        self.instruments.indexed(to - from + 1);
        debug!(from, to, blocks = to - from + 1, "range indexed");

        Ok(Iteration::Progressed { cursor_at: to })
    }

    /// `None` when the header extends what was folded; a fork is unwound here.
    async fn verify_ancestry(
        &mut self,
        header: &BlockHeader,
        safe_head: Option<u64>,
    ) -> Result<Option<Iteration>, String> {
        let verdict = self
            .reorg_detector
            .inspect(header, safe_head)
            .await
            .map_err(describe)?;

        match verdict {
            Verdict::Continuous => Ok(None),
            Verdict::Unrecoverable { reason } => Ok(Some(Iteration::Failed {
                reason: format!("reorg beyond the detector's reach: {reason}"),
            })),
            Verdict::Reorg {
                first_invalid_block,
                last_invalid_block,
                last_valid_hash,
            } => self
                .unwind(
                    first_invalid_block,
                    last_invalid_block,
                    last_valid_hash,
                    Phase::Inspect,
                    Some(header.number),
                )
                .await
                .map(Some),
        }
    }

    /// Processors, then the cursor, then the rewind: the rewind discards the
    /// headers a restart needs to find this fork again if the save is refused.
    async fn unwind(
        &mut self,
        first_invalid_block: u64,
        last_invalid_block: u64,
        last_valid_hash: B256,
        phase: Phase,
        at_block: Option<u64>,
    ) -> Result<Iteration, String> {
        self.instruments
            .reorg(first_invalid_block, last_invalid_block, phase, at_block);

        if let Some(refusal) = self
            .dispatch(Operation::Reorg, first_invalid_block, last_invalid_block)
            .await
        {
            return Ok(refusal);
        }

        let last_valid_block = first_invalid_block.checked_sub(1);
        match last_valid_block {
            Some(block) => self.save_cursor(block, last_valid_hash).await?,
            // Genesis itself was replaced, so there is no cursor below it to save.
            None => self.cursor = None,
        }

        self.reorg_detector
            .rewind_to(last_valid_block)
            .await
            .map_err(describe)?;

        Ok(Iteration::Progressed {
            cursor_at: last_valid_block.unwrap_or(0),
        })
    }

    async fn dispatch(&mut self, operation: Operation, from: u64, to: u64) -> Option<Iteration> {
        let result = dispatch::to_processors(
            &self.processors,
            operation,
            from,
            to,
            &self.cancel,
            &self.instruments,
        )
        .await;

        match result {
            Dispatch::Ok => None,
            Dispatch::Failed { reason, .. } => Some(Iteration::Failed { reason }),
            Dispatch::Retry {
                processor,
                reason,
                narrow_range,
            } => {
                if narrow_range {
                    self.narrow_range(&processor);
                }

                Some(Iteration::Retry { reason })
            }
        }
    }

    /// Never restored: a defect kept on purpose, see `docs/rust-migration.md`.
    fn narrow_range(&mut self, processor: &str) {
        self.effective_max_range = (self.effective_max_range / 2).max(1);
        self.instruments.narrowed_to(self.effective_max_range);
        warn!(
            processor,
            range_size = self.effective_max_range,
            "processor asked to narrow the range"
        );
    }

    /// The commit point, written last; the in-memory copy only follows an accepted save.
    async fn save_cursor(&mut self, last_block: u64, last_hash: B256) -> Result<(), String> {
        let cursor = Cursor {
            chain_id: self.options.chain_id,
            last_block,
            last_hash,
        };

        self.cursor_store.save(&cursor).await.map_err(describe)?;
        self.cursor = Some(cursor);

        Ok(())
    }
}
