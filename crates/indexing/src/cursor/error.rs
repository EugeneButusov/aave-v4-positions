//! What a cursor read refuses with.

/// A status that cannot be served.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Postgres would not answer: no connection to be had, or a query it
    /// refused. Which of the two is in the cause, and `ops`'s chain formatter is
    /// what puts it on the line the API logs.
    #[error("the sync status read failed")]
    Read(#[from] postgres::Error),

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
