//! What a price read refuses with.

/// A dimension that cannot be served.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No connection to take the query.
    #[error("no connection to Postgres")]
    Unavailable(#[from] postgres::Error),

    /// The server refused the query.
    #[error("the reserve price query failed")]
    QueryFailed(#[from] postgres::tokio_postgres::Error),

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
