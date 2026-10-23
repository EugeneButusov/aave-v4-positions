//! What a cursor read or write refuses with.

/// A cursor that cannot be read, or an advance that cannot be recorded.
///
/// **No `#[from]`**, so `?` on a `postgres::Error` does not compile here: the
/// conversion would have to guess which of the two directions it was.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Postgres would not answer: no connection to be had, or a statement it
    /// refused. Which of the two is in the cause.
    #[error("the cursor read failed")]
    ReadFailed(#[source] postgres::Error),

    /// The indexer did not advance. Its range is re-dispatched rather than
    /// skipped, so this is a stall and never lost blocks.
    #[error("the cursor advance could not be recorded")]
    WriteFailed(#[source] postgres::Error),

    /// A column held something its type says it cannot.
    ///
    /// Unreachable from a table the migrations built: `last_hash` carries a
    /// `CHECK (last_hash ~ '^0x[0-9a-f]{64}$')`.
    #[error("{column} is not a {expected}: {value}")]
    Malformed {
        column: &'static str,
        expected: &'static str,
        value: String,
    },
}
