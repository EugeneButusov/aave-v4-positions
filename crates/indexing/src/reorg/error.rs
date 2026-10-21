//! What the detector and its window refuse with.

/// One enum for both ports, because they are never held apart: a detector owns
/// a window, and every call into either lands in the same place — the loop,
/// which retries all of it.
///
/// [`Chain`](Self::Chain) is the detector's alone and [`Malformed`](Self::Malformed)
/// the window's; a reader of either signature can see which from the variant.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Postgres would not answer: no connection to be had, or a statement it
    /// refused. Which of the two is in the cause.
    #[error("the header window could not be read or written")]
    Failed(#[from] postgres::Error),

    /// A column held something its type says it cannot.
    ///
    /// Unreachable from a table the migrations built: `hash` and `parent_hash`
    /// each carry a `CHECK (… ~ '^0x[0-9a-f]{64}$')`. It refuses rather than
    /// substitutes, because a window read positionally would place a fork from
    /// a header it had to invent.
    #[error("{column} is not a {expected}: {value}")]
    Malformed {
        column: &'static str,
        expected: &'static str,
        value: String,
    },

    /// Walking a fork and vetting a resume point both read headers.
    #[error("the chain could not be read while placing a fork")]
    Chain(#[from] crate::chain::Error),
}
