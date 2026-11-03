use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::{Instrument as _, info, warn};

use crate::engine::metrics::Instruments;
use crate::engine::pipeline::Pipeline;
use crate::engine::status::IndexerStatus;
use crate::engine::step::Step;
use crate::engine::{Iteration, Options, pacing};

/// Walks the chain and hands ranges to the processors, one iteration at a time.
///
/// This is the driver: when an iteration runs, what it is recorded as, and how
/// long the loop waits after it. What an iteration does is `Step`'s.
pub struct Indexer {
    options: Options,
    processor_count: usize,
    status: Arc<IndexerStatus>,
    instruments: Instruments,
    cancel: CancellationToken,
    step: Step,
}

impl Indexer {
    #[must_use]
    pub fn new(pipeline: Pipeline) -> Self {
        Self {
            options: pipeline.options,
            processor_count: pipeline.processors.len(),
            status: Arc::clone(&pipeline.status),
            instruments: pipeline.instruments.clone(),
            cancel: pipeline.cancel.clone(),
            step: Step::new(pipeline),
        }
    }

    /// Until the cancellation token fires, or until an iteration is terminal.
    ///
    /// Sequential on purpose: each iteration depends on the last having
    /// finished, and overlapping two would race the cursor.
    pub async fn run(&mut self) {
        info!(
            chain_id = %self.options.chain_id,
            processors = self.processor_count,
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
            let delay = pacing::delay_after(
                &result,
                self.options.poll_interval,
                self.status.consecutive_failures(),
            );
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
        let outcome = self.step.advance().instrument(span.clone()).await;
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
}
