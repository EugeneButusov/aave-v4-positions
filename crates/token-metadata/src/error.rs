//! What a label read refuses with.

/// A dimension that cannot be served.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Postgres would not answer: no connection to be had, or a query it
    /// refused. Which of the two is in the cause, and `ops`'s chain formatter is
    /// what puts it on the line the API logs.
    #[error("the token label read failed")]
    Read(#[from] postgres::Error),

    /// A column held something its type says it cannot.
    ///
    /// Unreachable from a table the migrations built: `token` carries a
    /// `CHECK (token ~ '^0x[0-9a-f]{40}$')`, so the parse below can only fail
    /// on a row the constraint would have refused.
    #[error("{column} is not a {expected}: {value}")]
    Malformed {
        column: &'static str,
        expected: &'static str,
        value: String,
    },
}
