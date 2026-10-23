/// What constructing or probing the ClickHouse client can fail at.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the ClickHouse URL is invalid")]
    BadUrl {
        #[source]
        source: url::ParseError,
    },

    #[error("the ClickHouse URL must use HTTPS")]
    InsecureTransport,

    #[error("the ClickHouse URL scheme {scheme:?} is not supported")]
    UnsupportedScheme { scheme: String },

    /// Its own variant rather than the driver's message because this error can
    /// reach an unauthenticated readiness response, while `BadResponse` quotes
    /// the server's response verbatim.
    #[error("could not reach ClickHouse")]
    Unreachable {
        #[source]
        source: clickhouse::error::Error,
    },
}
