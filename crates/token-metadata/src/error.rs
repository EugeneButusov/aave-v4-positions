//! What a label read refuses with.

/// A dimension that cannot be served.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No connection to take the query.
    #[error("no connection to Postgres")]
    Unavailable(#[from] postgres::Error),

    /// The server refused the query.
    #[error("the token metadata query failed")]
    QueryFailed(#[from] tokio_postgres::Error),

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
