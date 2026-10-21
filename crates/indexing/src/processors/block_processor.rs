use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

/// Whatever a processor failed with: this crate cannot enumerate another's
/// failures and only ever reads the message off one.
pub type Failure = Box<dyn std::error::Error + Send + Sync>;

/// Three answers rather than two, because retrying and giving up are different
/// instructions to the caller and an error type cannot say which.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum Outcome {
    Ok,

    /// The cursor holds and the caller re-dispatches the same range, with no
    /// attempt limit: a wedged pipeline surfaces on readiness.
    Retry {
        reason: String,

        /// For a provider that refused this range's width, not for a rate
        /// limit: narrowing answers the first and does nothing about the second.
        narrow_range: bool,
    },

    /// Terminal: the loop stops and readiness fails.
    Failed {
        reason: String,
    },
}

/// One stage of the pipeline, given a range of blocks.
///
/// **Must be idempotent.** A later processor asking for a retry re-invokes the
/// earlier ones that already succeeded, and a crash between a processor's write
/// and the cursor advance replays the range.
#[async_trait]
pub trait BlockProcessor: Send + Sync {
    /// What a `--processors` filter matches, and what labels this processor's
    /// span and duration metric — so it has to be low cardinality.
    fn name(&self) -> &str;

    /// Index `from..=to`, both inclusive.
    ///
    /// `cancel` fires when shutdown begins; ignoring it is safe and wastes the
    /// work, but it will not be awaited past the drain window.
    ///
    /// # Errors
    ///
    /// [`Failure`] is read as [`Outcome::Retry`] and never as terminal: here a
    /// transient fault and a defect are indistinguishable, and calling one
    /// terminal would let a passing bug stop indexing for good.
    async fn on_block_range(
        &self,
        from: u64,
        to: u64,
        cancel: &CancellationToken,
    ) -> Result<Outcome, Failure>;

    /// Discard everything derived from `from..=to`, both inclusive. It runs
    /// again on the same range whenever the step could not be completed, which
    /// a discard can afford: discarding twice is discarding once.
    ///
    /// # Errors
    ///
    /// As [`on_block_range`](Self::on_block_range).
    async fn on_reorg(
        &self,
        from: u64,
        to: u64,
        cancel: &CancellationToken,
    ) -> Result<Outcome, Failure>;
}
