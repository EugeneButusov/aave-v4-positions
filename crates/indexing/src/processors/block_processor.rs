use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

/// Whatever a processor failed with. Boxed because this crate cannot enumerate
/// another's failures and only ever reads the message off one.
pub type Failure = Box<dyn std::error::Error + Send + Sync>;

/// A three-way answer, returned rather than thrown, because `retry` and
/// `failed` are different instructions to the caller and an error type cannot
/// say which.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum Outcome {
    Ok,

    /// Transient: the cursor holds, the caller backs off and re-dispatches the
    /// same range. Unbounded, because an outage should recover on its own — a
    /// wedged pipeline surfaces on readiness, not by giving up.
    Retry {
        reason: String,

        /// Ask for a narrower range next time. For a provider that refused this
        /// one's width, not for a rate limit: narrowing answers the first and
        /// does nothing about the second.
        narrow_range: bool,
    },

    /// Terminal: more attempts cannot help. The loop stops and readiness fails.
    Failed {
        reason: String,
    },
}

/// One stage of the pipeline, given a range of blocks.
///
/// **Must be idempotent.** Dispatch is at-least-once per processor per range,
/// for two reasons that are not going away: a later processor asking for a
/// retry re-invokes the earlier ones that already succeeded, and a crash
/// between a processor's own write and the cursor advance replays the range.
#[async_trait]
pub trait BlockProcessor: Send + Sync {
    /// What a `--processors` filter matches, and what labels this processor's
    /// span and duration metric. Low cardinality.
    fn name(&self) -> &str;

    /// Index `from..=to`, both inclusive.
    ///
    /// `cancel` fires when shutdown begins. Honouring it is optional — ignoring
    /// it is safe and wastes the work — but it will not be awaited past the
    /// drain window.
    ///
    /// # Errors
    ///
    /// [`Failure`] is read as [`Outcome::Retry`] and never as terminal: at this
    /// seam a transient fault and a defect are indistinguishable, and calling
    /// one terminal would let a passing bug stop indexing for good.
    async fn on_block_range(
        &self,
        from: u64,
        to: u64,
        cancel: &CancellationToken,
    ) -> Result<Outcome, Failure>;

    /// Discard everything derived from `from..=to`, both inclusive. The caller
    /// then rewinds the cursor to `from - 1` and dispatches replacements
    /// through [`on_block_range`](Self::on_block_range), so this runs again on
    /// the same range whenever the step could not be completed — it is a
    /// discard, and discarding twice is discarding once.
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
