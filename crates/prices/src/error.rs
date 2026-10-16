//! What a price read refuses with.

/// A dimension that cannot be served.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Postgres would not answer: no connection to be had, or a query it
    /// refused. Which of the two is in the cause, and `ops`'s chain formatter is
    /// what puts it on the line the API logs.
    #[error("the reserve price read failed")]
    ReadFailed(#[from] postgres::Error),

    /// A column held something its type says it cannot.
    ///
    /// The wide columns arrive as text and are parsed here, because the
    /// alternative is a driver type that has already lost the tail past 2^53
    /// (§7.5). Unreachable from a table the migrations built: `spoke` carries a
    /// lower-cased-hex `CHECK`, and both `numeric(78,0)` columns hold nothing a
    /// `uint256` cannot.
    #[error("{column} is not a {expected}: {value}")]
    Malformed {
        column: &'static str,
        expected: &'static str,
        value: String,
    },
}
