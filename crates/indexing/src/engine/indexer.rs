use std::sync::Arc;
use std::time::Duration;

use alloy_primitives::B256;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument as _, debug, info, warn};

use crate::chain::{BlockHeader, ChainClient};
use crate::cursor::{Cursor, CursorStore};
use crate::engine::Iteration;
use crate::engine::dispatch::{self, Dispatch, Operation};
use crate::engine::metrics::{Instruments, Phase};
use crate::engine::status::IndexerStatus;
use crate::processors::BlockProcessor;
use crate::reorg::{ReorgDetector, Verdict};

/// Constants rather than configuration: retries are unbounded, and the stall
/// signal rather than the delay is what makes a wedged loop visible, so there is
/// nothing here an operator would usefully tune.
const RETRY_BASE: Duration = Duration::from_millis(500);
const RETRY_MAX: Duration = Duration::from_secs(30);

/// How long a shutdown waits for an iteration that ignored its cancellation.
///
/// Exists so a processor that does not honour the token cannot make the drain
/// outlive the container's termination grace period.
pub const SHUTDOWN_DRAIN: Duration = Duration::from_secs(10);

/// What the loop reads, and nothing more.
///
/// Four fields where the shape being ported has nine, because the other five
/// are read by its collaborators rather than by it: the finality depth is the
/// detector's, the provider list and timeout the chain client's, and the stall
/// threshold the readiness check's. Sharing one options object across all of
/// them is what made the loop look as though it decided any of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub chain_id: u32,

    /// The floor, used only when the cursor store has nothing.
    pub start_block: u64,

    /// Upper bound on one dispatched range, halved at runtime if a processor
    /// asks.
    pub max_range_size: u64,

    /// How long to wait after catching up.
    pub poll_interval: Duration,
}

/// Walks the chain and hands ranges to the processors.
///
/// **The loop knows about block numbers and nothing else.** It holds no notion
/// of finality, of what a fork looks like, or of what a processor does with a
/// range; each of those sits behind a port, and every chain-shape question is
/// answered by asking rather than by arithmetic here.
///
/// Each iteration takes one of two paths, chosen fresh from the head every time
/// rather than latched as a mode:
///
/// - **At or below the detector's safe head**, blocks are settled, so they go
///   out in ranges of up to `max_range_size` with no per-block inspection. That
///   is what makes a backfill of the better part of a million blocks take
///   minutes.
/// - **Above it**, one block at a time, inspected before it is dispatched.
///
/// A range never straddles the boundary: half of it would be covered by a
/// finality assumption and half not, and the single hash committed for its top
/// would sit inside the reorg window with no ancestry beneath it.
pub struct Indexer {
    options: Options,
    chain: Arc<dyn ChainClient>,
    detector: Box<dyn ReorgDetector>,
    cursors: Box<dyn CursorStore>,
    processors: Vec<Box<dyn BlockProcessor>>,
    status: Arc<IndexerStatus>,
    instruments: Instruments,
    cancel: CancellationToken,

    cursor: Option<Cursor>,
    chain_id_verified: bool,
    bootstrapped: bool,
    effective_max_range: u64,
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
    pub chain: Arc<dyn ChainClient>,
    pub detector: Box<dyn ReorgDetector>,
    pub cursors: Box<dyn CursorStore>,

    /// Dispatched in this order, stopping at the first that does not accept a
    /// range.
    pub processors: Vec<Box<dyn BlockProcessor>>,

    /// Shared with the gauges and the readiness check.
    pub status: Arc<IndexerStatus>,
    pub instruments: Instruments,
    pub cancel: CancellationToken,
}

impl Indexer {
    #[must_use]
    pub fn new(pipeline: Pipeline) -> Self {
        let Pipeline {
            options,
            chain,
            detector,
            cursors,
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
            chain,
            detector,
            cursors,
            processors,
            status,
            instruments,
            cancel,
        }
    }

    /// Until the cancellation token fires, or until an iteration is terminal.
    ///
    /// Sequential on purpose: each iteration depends on the last having
    /// finished, and overlapping two would race the cursor.
    pub async fn run(&mut self) {
        info!(
            chain_id = self.options.chain_id,
            processors = self.processors.len(),
            "indexing started"
        );

        while !self.cancel.is_cancelled() {
            let result = self.run_once().await;

            if let Iteration::Failed { reason } = &result {
                // Terminal by design. Nothing here resets it — a restart does,
                // which is the posture the rest of the service takes towards
                // bad configuration.
                tracing::error!(reason, "indexing stopped");

                return;
            }

            if let Iteration::Retry { reason } = &result {
                warn!(
                    attempt = self.status.consecutive_failures(),
                    reason, "iteration failed"
                );
            }

            // Wakes on cancellation too, so a quiet loop does not sit out its
            // poll interval before noticing.
            let delay = self.delay_for(&result);
            if !delay.is_zero() {
                tokio::select! {
                    () = tokio::time::sleep(delay) => {}
                    () = self.cancel.cancelled() => {}
                }
            }
        }

        self.status.stopped();
    }

    /// One pass, including the state transition it implies.
    ///
    /// Public because it is the unit a case drives: nearly every behaviour here
    /// can be asserted by calling this directly, with no timers and no
    /// background task to wind down.
    ///
    /// Terminal failure is checked here rather than only in [`run`](Self::run),
    /// so the state is right however this is driven and a caller cannot
    /// resurrect a stopped indexer by calling again. **The span is opened after
    /// that check**, so a stopped loop answering "still stopped" every few
    /// seconds is not an iteration — emitting a span for each would fill a trace
    /// backend with nothing.
    pub async fn run_once(&mut self) -> Iteration {
        if self.status.is_failed() {
            return Iteration::Failed {
                reason: self
                    .status
                    .failure_reason()
                    .unwrap_or_else(|| "indexing stopped".to_owned()),
            };
        }

        let span = tracing::info_span!(
            "iterate",
            otel.name = "indexer.iterate",
            otel.status_code = tracing::field::Empty,
            indexer.outcome = tracing::field::Empty,
            indexer.cursor.block = tracing::field::Empty,
        );
        // `.instrument` rather than an entered guard: a guard held across an
        // await keeps the span entered while another task runs on the thread,
        // so every span opened there would nest under this iteration.
        let started = tokio::time::Instant::now();
        let outcome = self.iterate().instrument(span.clone()).await;
        let result = self.record(outcome);
        let elapsed = started.elapsed();

        span.record("indexer.outcome", result.outcome());
        match &result {
            Iteration::Progressed { cursor_at } => {
                span.record("indexer.cursor.block", *cursor_at);
            }
            // A retry is routine to the loop and an error to a trace backend,
            // and the second framing is the useful one: it is what makes the
            // failing iterations filterable out of thousands of healthy ones.
            Iteration::Retry { .. } | Iteration::Failed { .. } => {
                span.record("otel.status_code", "Error");
            }
            Iteration::Idle => {}
        }

        self.instruments.iteration(&result, elapsed);

        result
    }

    /// Translates what happened into a state transition.
    fn record(&self, result: Iteration) -> Iteration {
        match &result {
            Iteration::Failed { reason } => self.status.failed(reason.clone()),
            Iteration::Retry { reason } => self.status.retried(reason.clone()),
            Iteration::Progressed { cursor_at } => self.status.progressed(*cursor_at),
            Iteration::Idle => self.status.idled(),
        }

        result
    }

    async fn iterate(&mut self) -> Iteration {
        match self.attempt().await {
            Ok(result) => result,
            // Reached only by a fault outside a processor — a failed read, a
            // cursor store refusing. Retries are unbounded, so a genuine defect
            // keeps failing here and surfaces through the stall signal rather
            // than being mistaken for a terminal condition.
            Err(reason) => Iteration::Retry {
                reason: format!("unhandled: {reason}"),
            },
        }
    }

    /// The iteration proper, with every refusal carried out as an `Err` so the
    /// one place above turns all of them into the same retry.
    async fn attempt(&mut self) -> Result<Iteration, String> {
        if let Some(decided) = self.verify_chain_id().await? {
            return Ok(decided);
        }

        if let Some(decided) = self.bootstrap().await? {
            return Ok(decided);
        }

        // Clamped to the high-water mark inside the status: a failover can
        // otherwise make the head appear to move backwards.
        let head = self
            .status
            .observe_head(self.chain.head_block_number().await.map_err(describe)?);

        let next = self
            .cursor
            .map_or(self.options.start_block, |cursor| cursor.last_block + 1);

        if next > head {
            return Ok(Iteration::Idle);
        }

        // The boundary decides how wide to dispatch and nothing else. At or
        // below it the blocks are settled, so they go out in ranges; above it,
        // one at a time. A range never straddles it.
        let safe_head = self.detector.safe_head(head).await.map_err(describe)?;
        let to = match safe_head {
            Some(settled) if next <= settled => (next + self.effective_max_range - 1).min(settled),
            _ => next,
        };

        self.index_range(next, to).await
    }

    /// Checked once, lazily, rather than at construction: a boot that reaches
    /// out to a node turns a provider outage into a crash loop, where what is
    /// wanted is a pod that starts and reports not-ready.
    async fn verify_chain_id(&mut self) -> Result<Option<Iteration>, String> {
        if self.chain_id_verified {
            return Ok(None);
        }

        let actual = self.chain.chain_id().await.map_err(describe)?;

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

    /// Loads the cursor and lets the detector vet the resume point.
    ///
    /// `bootstrapped` is set only once the whole step succeeded, so a transient
    /// failure re-runs it rather than leaving the detector half-primed.
    async fn bootstrap(&mut self) -> Result<Option<Iteration>, String> {
        if self.bootstrapped {
            return Ok(None);
        }

        let stored = self
            .cursors
            .load(self.options.chain_id)
            .await
            .map_err(describe)?;
        let verdict = self
            .detector
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
                // The chain forked while the process was down. Nothing else
                // would notice: the detector's window starts empty, so without
                // this the loop would carry on from the cursor onto a branch
                // that lost.
                self.record_reorg(
                    first_invalid_block,
                    last_invalid_block,
                    Phase::Bootstrap,
                    None,
                );
                let rewound = self
                    .apply_reorg(first_invalid_block, last_invalid_block, last_valid_hash)
                    .await?;

                if !matches!(rewound, Iteration::Progressed { .. }) {
                    return Ok(Some(rewound));
                }

                self.bootstrapped = true;

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

    /// Indexes `from..=to`, having asked the detector about it first.
    ///
    /// One path for settled and unsettled alike. The width is the only thing
    /// that differs, and the width is all the loop decides: a wide range means
    /// one header read for many blocks, so the top is the only header there is
    /// to anchor on or ask about. Whether that header needs checking is the
    /// detector's call — it knows where the boundary is, and a settled top it
    /// waves through without looking.
    ///
    /// Reading the top header even for a settled range costs one call per range
    /// and is what lets `Cursor::last_hash` mean the same thing always, so the
    /// detector can re-anchor from the cursor alone.
    async fn index_range(&mut self, from: u64, to: u64) -> Result<Iteration, String> {
        let header = self.chain.block_header(to).await.map_err(describe)?;

        if let Some(decided) = self.verify_ancestry(&header).await? {
            return Ok(decided);
        }

        if let Some(refusal) = self.dispatch(Operation::BlockRange, from, to).await {
            return Ok(refusal);
        }

        self.detector.commit(&header).await.map_err(describe)?;
        self.save_cursor(header.number, header.hash).await?;

        self.instruments.indexed(to - from + 1);
        // `debug` and not `info`: at one iteration every few seconds a healthy
        // indexer would otherwise be the whole log stream. Before it existed an
        // empty range was completely silent.
        debug!(from, to, blocks = to - from + 1, "range indexed");

        Ok(Iteration::Progressed { cursor_at: to })
    }

    /// Asks the detector whether this header extends what was folded, and
    /// answers `None` when it does.
    ///
    /// **It does not only report.** A fork is unwound here, which dispatches the
    /// discard and moves the cursor, so the `Progressed` that comes back
    /// describes the rewind rather than the range the caller was about to index.
    async fn verify_ancestry(&mut self, header: &BlockHeader) -> Result<Option<Iteration>, String> {
        match self.detector.inspect(header).await.map_err(describe)? {
            Verdict::Continuous => Ok(None),
            Verdict::Unrecoverable { reason } => Ok(Some(Iteration::Failed {
                reason: format!("reorg beyond the detector's reach: {reason}"),
            })),
            Verdict::Reorg {
                first_invalid_block,
                last_invalid_block,
                last_valid_hash,
            } => {
                self.record_reorg(
                    first_invalid_block,
                    last_invalid_block,
                    Phase::Inspect,
                    Some(header.number),
                );

                self.apply_reorg(first_invalid_block, last_invalid_block, last_valid_hash)
                    .await
                    .map(Some)
            }
        }
    }

    /// Unwinds a fork. **Nothing is mutated until every processor has accepted
    /// the discard**, so a refusal there leaves the step entirely unapplied and
    /// retried.
    ///
    /// **The rewind is last, after the cursor is durable**, because it discards
    /// the headers that are the evidence of the abandoned branch. Rewinding
    /// first and then failing the save would leave the detector holding less
    /// than it started with, having committed to nothing; this way it holds
    /// more — stale headers above an already-correct cursor — which a bootstrap
    /// resolves by re-reporting the range for an idempotent second discard.
    async fn apply_reorg(
        &mut self,
        first_invalid_block: u64,
        last_invalid_block: u64,
        last_valid_hash: B256,
    ) -> Result<Iteration, String> {
        if let Some(refusal) = self
            .dispatch(Operation::Reorg, first_invalid_block, last_invalid_block)
            .await
        {
            return Ok(refusal);
        }

        let last_valid_block = first_invalid_block.checked_sub(1);

        match last_valid_block {
            Some(block) => self.save_cursor(block, last_valid_hash).await?,
            // Genesis itself was replaced. There is no cursor below it, so the
            // store is left alone and the window is cleared.
            None => self.cursor = None,
        }

        self.detector
            .rewind_to(last_valid_block)
            .await
            .map_err(describe)?;

        Ok(Iteration::Progressed {
            cursor_at: last_valid_block.unwrap_or(0),
        })
    }

    /// Hands the range over and translates the answer. `None` when every
    /// processor accepted it.
    ///
    /// Only the translation lives here: the retry policy is what makes the loop
    /// different from a one-shot backfill, and it is one line of it — a retry
    /// backs the loop off and is re-attempted forever, because an outage should
    /// recover by itself and a wedged processor is surfaced by the stall signal
    /// rather than by giving up.
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

    /// A reorg, said three ways at once.
    ///
    /// As a span **event** rather than a span of its own, because a fork is
    /// something that happened *inside* an iteration rather than a phase beside
    /// it. The histogram's own count is how often, so there is no separate
    /// counter; its values are how deep, which is the number worth watching
    /// against the finality depth.
    fn record_reorg(
        &self,
        first_invalid_block: u64,
        last_invalid_block: u64,
        phase: Phase,
        at_block: Option<u64>,
    ) {
        let depth = last_invalid_block - first_invalid_block + 1;

        self.instruments.reorg(depth, phase);

        // **Two emissions, because one cannot be both.** The span event's name
        // is taken from the message when there is one, so an event carrying the
        // sentence would be named after it and a trace backend could not query
        // for `reorg`. The first is the event; the second is the line.
        tracing::event!(
            name: "reorg",
            tracing::Level::WARN,
            depth,
            phase = phase.as_str(),
            first_invalid_block,
            last_invalid_block,
        );
        warn!(
            depth,
            phase = phase.as_str(),
            first_invalid_block,
            last_invalid_block,
            at_block,
            "reorg; blocks are stale"
        );
    }

    /// Halved, floored at 1, and **never restored**. Ported as it stands: see
    /// the preserved-defect table in `docs/rust-migration.md`.
    fn narrow_range(&mut self, processor: &str) {
        self.effective_max_range = (self.effective_max_range / 2).max(1);
        self.instruments.narrowed_to(self.effective_max_range);
        warn!(
            processor,
            range_size = self.effective_max_range,
            "processor asked to narrow the range"
        );
    }

    /// The single durable commit point, written last. A crash before it replays
    /// the range; a crash after it does not. The in-memory copy is updated only
    /// once the store accepted the write, so a refusing store leaves the loop
    /// pointing at the last position actually persisted.
    async fn save_cursor(&mut self, last_block: u64, last_hash: B256) -> Result<(), String> {
        let cursor = Cursor {
            chain_id: self.options.chain_id,
            last_block,
            last_hash,
        };

        self.cursors.save(&cursor).await.map_err(describe)?;
        self.cursor = Some(cursor);

        Ok(())
    }

    fn delay_for(&self, result: &Iteration) -> Duration {
        match result {
            Iteration::Progressed { .. } => Duration::ZERO,
            Iteration::Idle => self.options.poll_interval,
            // Ported as it stands, including that this arm is unreachable from
            // `run`, which returns before sleeping on a terminal failure.
            Iteration::Retry { .. } | Iteration::Failed { .. } => {
                backoff(self.status.consecutive_failures())
            }
        }
    }
}

/// How long to wait after the `failures`th consecutive failure.
///
/// 500ms doubling to a 30s ceiling. **No jitter**, ported as it stands: two
/// replicas retrying one dead provider stay in lockstep, and `docs/rust-migration.md`
/// carries that with the other three preserved defects.
///
/// A free function because the formula is a pure function of the count, and the
/// loop reaching for it through its own state is what made it look otherwise.
pub(super) fn backoff(failures: u32) -> Duration {
    let exponent = failures.saturating_sub(1).min(16);

    RETRY_MAX.min(RETRY_BASE * 2_u32.pow(exponent))
}

/// Every port's error, flattened to the message the loop puts on a line. The
/// chain formatter is what keeps the cause from being lost.
fn describe(error: impl std::error::Error) -> String {
    ops_cause(&error)
}

/// Spelled out here rather than linking `ops`, which pulls axum: this crate may
/// not reach a socket.
fn ops_cause(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut cause = error.source();

    while let Some(next) = cause {
        let text = next.to_string();
        if !message.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        cause = next.source();
    }

    message
}
