//! What a cursor read or write refuses with.

/// A cursor that cannot be read, or an advance that cannot be recorded.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Postgres would not answer: no connection to be had, or a statement it
    /// refused. Which of the two is in the cause, and `ops`'s chain formatter is
    /// what puts it on the line the API logs.
    ///
    /// Not `ReadFailed`, as its two sibling crates spell it: this is the one of
    /// the three whose port also writes.
    #[error("the cursor could not be read or written")]
    Failed(#[from] postgres::Error),

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
