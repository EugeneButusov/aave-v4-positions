//! What the detector and its window refuse with.

/// One enum for both ports, because they are never held apart: a detector owns
/// a window, and every call into either is retried by the same loop.
///
/// **No `#[from]` for `postgres::Error`**, so `?` on one does not compile here:
/// the conversion would have to guess which direction failed. `chain::Error`
/// keeps one, because reading the chain is the only thing that raises it.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Postgres would not answer: no connection to be had, or a statement it
    /// refused. Which of the two is in the cause.
    #[error("the header window read failed")]
    WindowReadFailed(#[source] postgres::Error),

    /// Nothing was retained or discarded, which the detector's ordering makes
    /// a replay rather than a gap: the cursor advances after a commit.
    #[error("the header window could not be written")]
    WindowWriteFailed(#[source] postgres::Error),

    /// Walking a fork and vetting a resume point both read headers.
    #[error("the chain could not be read while placing a fork")]
    ChainReadFailed(#[from] crate::chain::Error),

    /// A column held something its type says it cannot.
    ///
    /// Unreachable from a table the migrations built: `hash` and `parent_hash`
    /// each carry a `CHECK (… ~ '^0x[0-9a-f]{64}$')`. Refused rather than
    /// substituted, because a window is read positionally and an invented
    /// header would place a fork.
    #[error("{column} is not a {expected}: {value}")]
    Malformed {
        column: &'static str,
        expected: &'static str,
        value: String,
    },

    /// The two reorg transitions a window double can refuse, apart so a case
    /// names the transition it scripted rather than taking whichever came back.
    /// A commit may clear a broken run before retaining its new anchor, so one
    /// generic refusal would not prove which transition surfaced. The driver
    /// variants above are out of reach: every `tokio_postgres::Error`
    /// constructor is crate-private bar one marked `#[doc(hidden)]`.
    #[cfg(test)]
    #[error("the reorg window refused to retain a processed header")]
    RetentionRefused,

    #[cfg(test)]
    #[error("the reorg window refused to apply a rewind")]
    RewindRefused,
}
