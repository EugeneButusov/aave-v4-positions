//! What a cursor read refuses with.

/// A status that cannot be served.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No connection to take the query.
    #[error("no connection to Postgres")]
    Unavailable(#[from] postgres::Error),

    /// The server refused the query.
    #[error("the sync status query failed")]
    QueryFailed(#[from] tokio_postgres::Error),

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
