//! Handing one range to every processor, in order, stopping at the first that
//! does not accept it.
//!
//! Here rather than beside the port, because the span and the duration metric
//! per processor are recorded from inside this loop: dispatch is the only place
//! that holds the name, the operation, the range and the outcome at once.

use tokio_util::sync::CancellationToken;
use tracing::Instrument as _;

use crate::engine::metrics::Instruments;
use crate::processors::{BlockProcessor, Outcome};

/// Which of the two things a range can be handed over for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operation {
    BlockRange,
    Reorg,
}

impl Operation {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::BlockRange => "block-range",
            Self::Reorg => "reorg",
        }
    }
}

/// What a whole dispatch amounted to, as opposed to [`Outcome`], which is one
/// processor's answer.
///
/// Three differences, all so the caller does not re-derive them: `narrow_range`
/// is normalised to a plain boolean, `reason` is already composed for a human,
/// and `processor` names who decided it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub(crate) enum Dispatch {
    Ok,
    Retry {
        processor: String,
        reason: String,
        narrow_range: bool,
    },
    Failed {
        processor: String,
        reason: String,
    },
}

/// Runs the processors in registration order, stopping at the first non-`Ok`.
///
/// Fail-fast means an earlier processor is re-invoked when a later one asks to
/// retry, which is why the port requires idempotence.
///
/// Shared with the backfill runner when it arrives. The two differ in what they
/// do with the answer — one backs off forever, the other gives up after a
/// bounded number of attempts — but not in how a range reaches a processor, and
/// a second copy of that is exactly what would drift.
pub(crate) async fn to_processors(
    processors: &[Box<dyn BlockProcessor>],
    operation: Operation,
    from: u64,
    to: u64,
    cancel: &CancellationToken,
    instruments: &Instruments,
) -> Dispatch {
    for processor in processors {
        let name = processor.name().to_owned();
        let answer = observed(processor.as_ref(), operation, from, to, cancel, instruments).await;

        match answer {
            // **A refusal is a retry, never a failure.** The two are
            // indistinguishable at this level, and calling one terminal would
            // let a transient bug stop indexing for good.
            Err(error) => {
                return Dispatch::Retry {
                    reason: format!("{name} refused: {error}"),
                    processor: name,
                    narrow_range: false,
                };
            }
            Ok(Outcome::Failed { reason }) => {
                return Dispatch::Failed {
                    reason: format!("{name}: {reason}"),
                    processor: name,
                };
            }
            Ok(Outcome::Retry {
                reason,
                narrow_range,
            }) => {
                return Dispatch::Retry {
                    reason: format!("{name}: {reason}"),
                    processor: name,
                    narrow_range,
                };
            }
            Ok(Outcome::Ok) => {}
        }
    }

    Dispatch::Ok
}

/// One processor's turn, in a span and on the clock.
async fn observed(
    processor: &dyn BlockProcessor,
    operation: Operation,
    from: u64,
    to: u64,
    cancel: &CancellationToken,
    instruments: &Instruments,
) -> Result<Outcome, crate::processors::Failure> {
    let span = tracing::info_span!(
        "processor",
        otel.name = format!("processor.{}", processor.name()),
        otel.status_code = tracing::field::Empty,
        operation = operation.as_str(),
        block.from = from,
        block.to = to,
        outcome = tracing::field::Empty,
    );

    let started = tokio::time::Instant::now();
    let answer = match operation {
        Operation::BlockRange => processor.on_block_range(from, to, cancel),
        Operation::Reorg => processor.on_reorg(from, to, cancel),
    }
    .instrument(span.clone())
    .await;

    // In every arm, including the refusal: a processor that refuses becomes a
    // retry one level up, and it would be the one outcome missing from the
    // histogram otherwise.
    let outcome = match &answer {
        Ok(Outcome::Ok) => "ok",
        Ok(Outcome::Retry { .. }) => "retry",
        Ok(Outcome::Failed { .. }) => "failed",
        Err(_) => "refused",
    };

    instruments.processor(processor.name(), operation, outcome, started.elapsed());
    span.record("outcome", outcome);

    if outcome != "ok" {
        span.record("otel.status_code", "Error");
    }

    answer
}
